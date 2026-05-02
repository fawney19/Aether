use aether_data_contracts::repository::candidates::{
    RequestCandidateStatus, StoredRequestCandidate,
};
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;

const FAILURE_COOLDOWN_WINDOW_SECS: u64 = 60;
const FAILURE_COOLDOWN_THRESHOLD: usize = 2;
const ACTIVE_REQUEST_WINDOW_SECS: u64 = 300;
pub const PROVIDER_KEY_RPM_WINDOW_SECS: u64 = 60;
const PROBE_PHASE_REQUESTS: u32 = 100;
const PROBE_RESERVATION_RATIO: f64 = 0.1;
const STABLE_MIN_RESERVATION_RATIO: f64 = 0.1;
const STABLE_MAX_RESERVATION_RATIO: f64 = 0.35;
const SUCCESS_COUNT_FOR_FULL_CONFIDENCE: u32 = 50;
const COOLDOWN_HOURS_FOR_FULL_CONFIDENCE: f64 = 24.0;
const LOW_LOAD_THRESHOLD: f64 = 0.5;
const HIGH_LOAD_THRESHOLD: f64 = 0.8;
const ENFORCEMENT_CONFIDENCE_THRESHOLD: f64 = 0.6;
const CONFIDENCE_DECAY_PER_MINUTE: f64 = 0.005;
const MIN_CONSISTENT_OBSERVATIONS: usize = 3;
const MIN_HEADER_CONFIRMATIONS: usize = 2;
const OBSERVATION_CONSISTENCY_THRESHOLD: f64 = 0.3;
const HEALTH_DEGRADED_THRESHOLD: f64 = 0.8;
const HEALTH_LOW_THRESHOLD: f64 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProviderKeyHealthBucket {
    Low,
    Degraded,
    Healthy,
}

impl ProviderKeyHealthBucket {
    fn from_score(score: f64) -> Self {
        let score = score.clamp(0.0, 1.0);
        if score < HEALTH_LOW_THRESHOLD {
            return Self::Low;
        }
        if score < HEALTH_DEGRADED_THRESHOLD {
            return Self::Degraded;
        }
        Self::Healthy
    }
}

pub fn is_candidate_in_recent_failure_cooldown(
    recent_candidates: &[StoredRequestCandidate],
    provider_id: &str,
    endpoint_id: &str,
    key_id: &str,
    now_unix_secs: u64,
) -> bool {
    let mut recent_failures = 0usize;

    for candidate in recent_candidates {
        if candidate.provider_id.as_deref() != Some(provider_id)
            || candidate.endpoint_id.as_deref() != Some(endpoint_id)
            || candidate.key_id.as_deref() != Some(key_id)
        {
            continue;
        }

        let observed_at_unix_secs = candidate
            .finished_at_unix_ms
            .or(candidate.started_at_unix_ms)
            .map(|ms| ms / 1000)
            .unwrap_or(candidate.created_at_unix_ms / 1000);
        if now_unix_secs.saturating_sub(observed_at_unix_secs) > FAILURE_COOLDOWN_WINDOW_SECS {
            continue;
        }

        match candidate.status {
            RequestCandidateStatus::Success => return false,
            RequestCandidateStatus::Failed | RequestCandidateStatus::Cancelled => {
                recent_failures += 1;
                if recent_failures >= FAILURE_COOLDOWN_THRESHOLD {
                    return true;
                }
            }
            RequestCandidateStatus::Available
            | RequestCandidateStatus::Unused
            | RequestCandidateStatus::Pending
            | RequestCandidateStatus::Streaming
            | RequestCandidateStatus::Skipped => {}
        }
    }

    false
}

pub fn count_recent_active_requests_for_provider(
    recent_candidates: &[StoredRequestCandidate],
    provider_id: &str,
    now_unix_secs: u64,
) -> usize {
    recent_candidates
        .iter()
        .filter(|candidate| candidate.provider_id.as_deref() == Some(provider_id))
        .filter(|candidate| is_recently_active(candidate, now_unix_secs))
        .count()
}

pub fn count_recent_active_requests_for_provider_key(
    recent_candidates: &[StoredRequestCandidate],
    key_id: &str,
    now_unix_secs: u64,
) -> usize {
    recent_candidates
        .iter()
        .filter(|candidate| candidate.key_id.as_deref() == Some(key_id))
        .filter(|candidate| is_recently_active(candidate, now_unix_secs))
        .count()
}

pub fn count_recent_active_requests_for_api_key(
    recent_candidates: &[StoredRequestCandidate],
    api_key_id: &str,
    now_unix_secs: u64,
) -> usize {
    recent_candidates
        .iter()
        .filter(|candidate| candidate.api_key_id.as_deref() == Some(api_key_id))
        .filter(|candidate| is_recently_active(candidate, now_unix_secs))
        .count()
}

pub fn effective_provider_key_rpm_limit(
    key: &StoredProviderCatalogKey,
    now_unix_secs: u64,
) -> Option<usize> {
    if let Some(limit) = key.rpm_limit.filter(|limit| *limit > 0) {
        return usize::try_from(limit).ok();
    }

    let learned_limit = key
        .learned_rpm_limit
        .filter(|limit| *limit > 0)
        .and_then(|limit| usize::try_from(limit).ok())?;
    if provider_key_adaptive_learning_confidence(key, now_unix_secs)
        < ENFORCEMENT_CONFIDENCE_THRESHOLD
    {
        return None;
    }

    Some(learned_limit)
}

pub fn count_recent_rpm_requests_for_provider_key(
    recent_candidates: &[StoredRequestCandidate],
    key_id: &str,
    now_unix_secs: u64,
) -> usize {
    count_recent_rpm_requests_for_provider_key_since(recent_candidates, key_id, now_unix_secs, None)
}

pub fn count_recent_rpm_requests_for_provider_key_since(
    recent_candidates: &[StoredRequestCandidate],
    key_id: &str,
    now_unix_secs: u64,
    reset_after_unix_secs: Option<u64>,
) -> usize {
    let mut attempted_count = 0usize;
    let mut max_observed = 0usize;

    for candidate in recent_candidates {
        if candidate.key_id.as_deref() != Some(key_id) {
            continue;
        }
        if !is_recent_rpm_observation(candidate, now_unix_secs) {
            continue;
        }
        let observed_at_unix_secs = candidate
            .started_at_unix_ms
            .map(|ms| ms / 1000)
            .unwrap_or(candidate.created_at_unix_ms / 1000);
        if reset_after_unix_secs.is_some_and(|reset_after| observed_at_unix_secs <= reset_after) {
            continue;
        }
        attempted_count += 1;
        max_observed = max_observed.max(candidate.concurrent_requests.unwrap_or_default() as usize);
    }

    max_observed.max(attempted_count)
}

pub fn provider_key_rpm_allows_request(
    key: &StoredProviderCatalogKey,
    recent_candidates: &[StoredRequestCandidate],
    now_unix_secs: u64,
    is_cached_user: bool,
) -> bool {
    provider_key_rpm_allows_request_since(
        key,
        recent_candidates,
        now_unix_secs,
        is_cached_user,
        None,
    )
}

pub fn provider_key_rpm_allows_request_since(
    key: &StoredProviderCatalogKey,
    recent_candidates: &[StoredRequestCandidate],
    now_unix_secs: u64,
    is_cached_user: bool,
    reset_after_unix_secs: Option<u64>,
) -> bool {
    let Some(effective_limit) = effective_provider_key_rpm_limit(key, now_unix_secs) else {
        return true;
    };
    if effective_limit == 0 {
        return false;
    }

    let current_usage = count_recent_rpm_requests_for_provider_key_since(
        recent_candidates,
        key.id.as_str(),
        now_unix_secs,
        reset_after_unix_secs,
    );
    if is_cached_user {
        return current_usage < effective_limit;
    }

    let available_for_new = available_provider_key_rpm_slots_for_new_user(
        key,
        current_usage,
        effective_limit,
        now_unix_secs,
    );
    current_usage < available_for_new
}

pub fn provider_key_health_score(key: &StoredProviderCatalogKey, api_format: &str) -> Option<f64> {
    let score = key
        .health_by_format
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|values| values.get(api_format))
        .and_then(serde_json::Value::as_object)
        .and_then(|payload| payload.get("health_score"))
        .and_then(json_value_as_f64)?;
    Some(score.clamp(0.0, 1.0))
}

pub fn aggregate_provider_key_health_score(key: &StoredProviderCatalogKey) -> Option<f64> {
    let health_by_format = key.health_by_format.as_ref()?.as_object()?;
    let mut scores = Vec::new();
    for payload in health_by_format.values() {
        let Some(score) = payload
            .as_object()
            .and_then(|payload| payload.get("health_score"))
            .and_then(json_value_as_f64)
        else {
            continue;
        };
        scores.push(score.clamp(0.0, 1.0));
    }
    scores.into_iter().reduce(f64::min)
}

pub fn effective_provider_key_health_score(
    key: &StoredProviderCatalogKey,
    api_format: &str,
) -> Option<f64> {
    provider_key_health_score(key, api_format).or_else(|| aggregate_provider_key_health_score(key))
}

pub fn provider_key_health_bucket(
    key: &StoredProviderCatalogKey,
    api_format: &str,
) -> Option<ProviderKeyHealthBucket> {
    effective_provider_key_health_score(key, api_format).map(ProviderKeyHealthBucket::from_score)
}

pub fn is_provider_key_circuit_open(key: &StoredProviderCatalogKey, api_format: &str) -> bool {
    key.circuit_breaker_by_format
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|values| values.get(api_format))
        .and_then(serde_json::Value::as_object)
        .and_then(|payload| payload.get("open"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn available_provider_key_rpm_slots_for_new_user(
    key: &StoredProviderCatalogKey,
    current_usage: usize,
    effective_limit: usize,
    now_unix_secs: u64,
) -> usize {
    let reservation_ratio =
        provider_key_dynamic_reservation_ratio(key, current_usage, effective_limit, now_unix_secs);
    usize::max(
        1,
        (effective_limit as f64 * (1.0 - reservation_ratio)).floor() as usize,
    )
}

fn provider_key_dynamic_reservation_ratio(
    key: &StoredProviderCatalogKey,
    current_usage: usize,
    effective_limit: usize,
    now_unix_secs: u64,
) -> f64 {
    let total_requests = provider_key_total_requests(key);
    if total_requests < PROBE_PHASE_REQUESTS {
        return PROBE_RESERVATION_RATIO;
    }

    let confidence = provider_key_reservation_confidence(key, now_unix_secs);
    let load_ratio = provider_key_load_ratio(current_usage, effective_limit);
    if load_ratio < LOW_LOAD_THRESHOLD {
        return STABLE_MIN_RESERVATION_RATIO;
    }
    if load_ratio < HIGH_LOAD_THRESHOLD {
        let load_factor =
            (load_ratio - LOW_LOAD_THRESHOLD) / (HIGH_LOAD_THRESHOLD - LOW_LOAD_THRESHOLD);
        return STABLE_MIN_RESERVATION_RATIO
            + confidence
                * load_factor
                * (STABLE_MAX_RESERVATION_RATIO - STABLE_MIN_RESERVATION_RATIO);
    }

    STABLE_MIN_RESERVATION_RATIO
        + confidence * (STABLE_MAX_RESERVATION_RATIO - STABLE_MIN_RESERVATION_RATIO)
}

fn provider_key_adaptive_learning_confidence(
    key: &StoredProviderCatalogKey,
    now_unix_secs: u64,
) -> f64 {
    if key.learned_rpm_limit.is_none() {
        return 0.0;
    }

    let base_confidence = provider_key_adaptive_base_confidence(key);
    if base_confidence <= 0.0 {
        return 0.0;
    }

    let time_decay = match key.last_429_at_unix_secs {
        Some(last_429_at_unix_secs) => {
            now_unix_secs.saturating_sub(last_429_at_unix_secs) as f64 / 60.0
                * CONFIDENCE_DECAY_PER_MINUTE
        }
        None => 1.0,
    };

    (base_confidence - time_decay).clamp(0.0, 1.0)
}

fn provider_key_adaptive_base_confidence(key: &StoredProviderCatalogKey) -> f64 {
    let history = provider_key_adjustment_history(key);
    for record in history.iter().rev() {
        if record.get("type").and_then(serde_json::Value::as_str) == Some("429_observation") {
            continue;
        }
        if let Some(confidence) = record.get("confidence").and_then(json_value_as_f64) {
            return confidence.clamp(0.0, 1.0);
        }
    }

    let (_, confidence) = evaluate_provider_key_observations(&history);
    if confidence > 0.0 {
        return confidence;
    }

    if key.learned_rpm_limit.is_some() {
        return 0.3;
    }

    0.0
}

fn evaluate_provider_key_observations(
    history: &[serde_json::Map<String, serde_json::Value>],
) -> (Option<u32>, f64) {
    let observations = history
        .iter()
        .filter(|record| {
            record.get("type").and_then(serde_json::Value::as_str) == Some("429_observation")
        })
        .collect::<Vec<_>>();
    if observations.is_empty() {
        return (None, 0.0);
    }

    let header_values = observations
        .iter()
        .filter_map(|record| provider_key_observation_u32(record, "upstream_limit"))
        .collect::<Vec<_>>();
    if header_values.len() >= MIN_HEADER_CONFIRMATIONS {
        let recent = provider_key_recent_tail(&header_values, MIN_HEADER_CONFIRMATIONS * 2);
        let last_n = provider_key_recent_tail(recent, MIN_HEADER_CONFIRMATIONS);
        if provider_key_observations_consistent(last_n) {
            return (None, 0.8);
        }
    }

    let local_values = observations
        .iter()
        .filter_map(|record| provider_key_observation_u32(record, "current_rpm"))
        .collect::<Vec<_>>();
    if local_values.len() >= MIN_CONSISTENT_OBSERVATIONS {
        let recent = provider_key_recent_tail(&local_values, MIN_CONSISTENT_OBSERVATIONS * 2);
        let last_n = provider_key_recent_tail(recent, MIN_CONSISTENT_OBSERVATIONS);
        if provider_key_observations_consistent(last_n) {
            return (None, 0.6);
        }
    }

    (None, 0.0)
}

fn provider_key_adjustment_history(
    key: &StoredProviderCatalogKey,
) -> Vec<serde_json::Map<String, serde_json::Value>> {
    key.adjustment_history
        .as_ref()
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_object)
        .cloned()
        .collect()
}

fn provider_key_observation_u32(
    record: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Option<u32> {
    record
        .get(field)
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
}

fn provider_key_observations_consistent(values: &[u32]) -> bool {
    let median = provider_key_median(values);
    median > 0.0
        && values.iter().all(|value| {
            (*value as f64 - median).abs() / median <= OBSERVATION_CONSISTENCY_THRESHOLD
        })
}

fn provider_key_median(values: &[u32]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }

    let mut sorted = values.iter().map(|value| *value as f64).collect::<Vec<_>>();
    sorted.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let midpoint = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        (sorted[midpoint - 1] + sorted[midpoint]) / 2.0
    } else {
        sorted[midpoint]
    }
}

fn provider_key_recent_tail<T>(values: &[T], limit: usize) -> &[T] {
    let keep_from = values.len().saturating_sub(limit);
    &values[keep_from..]
}

fn is_recently_active(candidate: &StoredRequestCandidate, now_unix_secs: u64) -> bool {
    if candidate.finished_at_unix_ms.is_some() {
        return false;
    }

    if !matches!(
        candidate.status,
        RequestCandidateStatus::Pending | RequestCandidateStatus::Streaming
    ) {
        return false;
    }

    let observed_at_unix_secs = candidate
        .started_at_unix_ms
        .map(|ms| ms / 1000)
        .unwrap_or(candidate.created_at_unix_ms / 1000);
    now_unix_secs.saturating_sub(observed_at_unix_secs) <= ACTIVE_REQUEST_WINDOW_SECS
}

fn is_recent_rpm_observation(candidate: &StoredRequestCandidate, now_unix_secs: u64) -> bool {
    if !candidate.status.is_attempted(candidate.started_at_unix_ms) {
        return false;
    }

    let observed_at_unix_secs = candidate
        .started_at_unix_ms
        .map(|ms| ms / 1000)
        .unwrap_or(candidate.created_at_unix_ms / 1000);
    now_unix_secs.saturating_sub(observed_at_unix_secs) <= PROVIDER_KEY_RPM_WINDOW_SECS
}

fn provider_key_total_requests(key: &StoredProviderCatalogKey) -> u32 {
    let request_count = key.request_count.unwrap_or_default();
    if request_count > 0 {
        return request_count;
    }

    let history_count = key
        .adjustment_history
        .as_ref()
        .and_then(serde_json::Value::as_array)
        .map(|values| values.len() as u32 * 10)
        .unwrap_or_default();
    key.concurrent_429_count.unwrap_or_default()
        + key.rpm_429_count.unwrap_or_default()
        + key.success_count.unwrap_or_default()
        + history_count
}

fn provider_key_load_ratio(current_usage: usize, effective_limit: usize) -> f64 {
    if effective_limit == 0 {
        return 0.0;
    }

    (current_usage as f64 / effective_limit as f64).min(1.0)
}

fn provider_key_reservation_confidence(key: &StoredProviderCatalogKey, now_unix_secs: u64) -> f64 {
    let request_count = key.request_count.unwrap_or_default() as f64;
    let success_count = key.success_count.unwrap_or_default() as f64;

    let success_score = if request_count >= SUCCESS_COUNT_FOR_FULL_CONFIDENCE as f64 {
        let success_rate = if request_count > 0.0 {
            success_count / request_count
        } else {
            0.0
        };
        success_rate * 0.4
    } else if request_count > 0.0 {
        let success_rate = success_count / request_count;
        let progress_ratio = request_count / SUCCESS_COUNT_FOR_FULL_CONFIDENCE as f64;
        success_rate * progress_ratio * 0.4
    } else {
        0.0
    };

    let cooldown_score = match key.last_429_at_unix_secs {
        Some(last_429_at_unix_secs) => {
            let hours_since_429 =
                now_unix_secs.saturating_sub(last_429_at_unix_secs) as f64 / 3600.0;
            (hours_since_429 / COOLDOWN_HOURS_FOR_FULL_CONFIDENCE).min(1.0) * 0.3
        }
        None => 0.3,
    };

    let stability_score = provider_key_stability_score(key);
    (success_score + cooldown_score + stability_score).min(1.0)
}

fn provider_key_stability_score(key: &StoredProviderCatalogKey) -> f64 {
    let Some(history) = key
        .adjustment_history
        .as_ref()
        .and_then(serde_json::Value::as_array)
    else {
        return 0.15;
    };
    if history.len() < 3 {
        return 0.15;
    }

    let recent = if history.len() > 5 {
        &history[history.len() - 5..]
    } else {
        history.as_slice()
    };
    let limits = recent
        .iter()
        .filter_map(|entry| entry.get("new_limit"))
        .filter_map(json_value_as_f64)
        .collect::<Vec<_>>();
    if limits.len() < 2 {
        return 0.15;
    }

    let mean = limits.iter().sum::<f64>() / limits.len() as f64;
    let variance = limits
        .iter()
        .map(|limit| {
            let delta = *limit - mean;
            delta * delta
        })
        .sum::<f64>()
        / (limits.len() as f64 - 1.0);
    let stability_ratio = (1.0 - variance / 10.0).max(0.0);
    stability_ratio * 0.3
}

fn json_value_as_f64(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|raw| raw as f64))
        .or_else(|| value.as_u64().map(|raw| raw as f64))
}

#[cfg(test)]
mod tests {
    use aether_data_contracts::repository::candidates::{
        RequestCandidateStatus, StoredRequestCandidate,
    };
    use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;

    use super::{
        aggregate_provider_key_health_score, count_recent_active_requests_for_api_key,
        count_recent_active_requests_for_provider, count_recent_active_requests_for_provider_key,
        count_recent_rpm_requests_for_provider_key,
        count_recent_rpm_requests_for_provider_key_since, effective_provider_key_health_score,
        effective_provider_key_rpm_limit, is_candidate_in_recent_failure_cooldown,
        is_provider_key_circuit_open, provider_key_health_bucket, provider_key_health_score,
        provider_key_rpm_allows_request, provider_key_rpm_allows_request_since,
        ProviderKeyHealthBucket,
    };

    fn stored_candidate(
        id: &str,
        status: RequestCandidateStatus,
        created_at_unix_secs: i64,
    ) -> StoredRequestCandidate {
        let created_at_unix_ms = created_at_unix_secs * 1000;
        StoredRequestCandidate::new(
            id.to_string(),
            format!("req-{id}"),
            None,
            None,
            None,
            None,
            0,
            0,
            Some("provider-a".to_string()),
            Some("endpoint-a".to_string()),
            Some("key-a".to_string()),
            status,
            None,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            created_at_unix_ms,
            Some(created_at_unix_ms),
            Some(created_at_unix_ms),
        )
        .expect("candidate should build")
    }

    fn provider_catalog_key(id: &str) -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            id.to_string(),
            "provider-a".to_string(),
            "primary".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("provider key should build")
    }

    #[test]
    fn cooldown_triggers_after_two_recent_failures() {
        let recent_candidates = vec![
            stored_candidate("one", RequestCandidateStatus::Failed, 95),
            stored_candidate("two", RequestCandidateStatus::Cancelled, 99),
        ];

        assert!(is_candidate_in_recent_failure_cooldown(
            &recent_candidates,
            "provider-a",
            "endpoint-a",
            "key-a",
            100,
        ));
    }

    #[test]
    fn recent_success_clears_cooldown() {
        let recent_candidates = vec![
            stored_candidate("one", RequestCandidateStatus::Failed, 95),
            stored_candidate("two", RequestCandidateStatus::Success, 99),
            stored_candidate("three", RequestCandidateStatus::Cancelled, 98),
        ];

        assert!(!is_candidate_in_recent_failure_cooldown(
            &recent_candidates,
            "provider-a",
            "endpoint-a",
            "key-a",
            100,
        ));
    }

    #[test]
    fn counts_only_recently_active_provider_requests() {
        let recent_candidates = vec![
            StoredRequestCandidate::new(
                "one".to_string(),
                "req-one".to_string(),
                None,
                Some("api-key-1".to_string()),
                None,
                None,
                0,
                0,
                Some("provider-a".to_string()),
                Some("endpoint-a".to_string()),
                Some("key-a".to_string()),
                RequestCandidateStatus::Pending,
                None,
                false,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                95,
                Some(95),
                None,
            )
            .expect("candidate should build"),
            StoredRequestCandidate::new(
                "two".to_string(),
                "req-two".to_string(),
                None,
                Some("api-key-1".to_string()),
                None,
                None,
                0,
                0,
                Some("provider-a".to_string()),
                Some("endpoint-a".to_string()),
                Some("key-a".to_string()),
                RequestCandidateStatus::Streaming,
                None,
                false,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                96,
                Some(96),
                None,
            )
            .expect("candidate should build"),
            StoredRequestCandidate::new(
                "three".to_string(),
                "req-three".to_string(),
                None,
                Some("api-key-1".to_string()),
                None,
                None,
                0,
                0,
                Some("provider-a".to_string()),
                Some("endpoint-a".to_string()),
                Some("key-a".to_string()),
                RequestCandidateStatus::Success,
                None,
                false,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                97,
                Some(97),
                Some(98),
            )
            .expect("candidate should build"),
        ];

        assert_eq!(
            count_recent_active_requests_for_provider(&recent_candidates, "provider-a", 100),
            2
        );
        assert_eq!(
            count_recent_active_requests_for_api_key(&recent_candidates, "api-key-1", 100),
            2
        );
    }

    #[test]
    fn provider_key_concurrency_counts_only_recent_active_requests() {
        let recent_candidates = vec![
            StoredRequestCandidate::new(
                "pending".to_string(),
                "req-pending".to_string(),
                None,
                Some("api-key-1".to_string()),
                None,
                None,
                0,
                0,
                Some("provider-a".to_string()),
                Some("endpoint-a".to_string()),
                Some("key-a".to_string()),
                RequestCandidateStatus::Pending,
                None,
                false,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                900_000,
                Some(900_000),
                None,
            )
            .expect("candidate should build"),
            StoredRequestCandidate::new(
                "streaming".to_string(),
                "req-streaming".to_string(),
                None,
                Some("api-key-1".to_string()),
                None,
                None,
                0,
                0,
                Some("provider-a".to_string()),
                Some("endpoint-a".to_string()),
                Some("key-a".to_string()),
                RequestCandidateStatus::Streaming,
                None,
                false,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                950_000,
                Some(950_000),
                None,
            )
            .expect("candidate should build"),
            StoredRequestCandidate::new(
                "finished".to_string(),
                "req-finished".to_string(),
                None,
                Some("api-key-1".to_string()),
                None,
                None,
                0,
                0,
                Some("provider-a".to_string()),
                Some("endpoint-a".to_string()),
                Some("key-a".to_string()),
                RequestCandidateStatus::Success,
                None,
                false,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                975_000,
                Some(975_000),
                Some(976_000),
            )
            .expect("candidate should build"),
            StoredRequestCandidate::new(
                "failed".to_string(),
                "req-failed".to_string(),
                None,
                Some("api-key-1".to_string()),
                None,
                None,
                0,
                0,
                Some("provider-a".to_string()),
                Some("endpoint-a".to_string()),
                Some("key-a".to_string()),
                RequestCandidateStatus::Failed,
                None,
                false,
                Some(429),
                None,
                Some("upstream failure".to_string()),
                Some(20),
                None,
                None,
                None,
                980_000,
                Some(980_000),
                Some(981_000),
            )
            .expect("candidate should build"),
            StoredRequestCandidate::new(
                "cancelled".to_string(),
                "req-cancelled".to_string(),
                None,
                Some("api-key-1".to_string()),
                None,
                None,
                0,
                0,
                Some("provider-a".to_string()),
                Some("endpoint-a".to_string()),
                Some("key-a".to_string()),
                RequestCandidateStatus::Cancelled,
                None,
                false,
                Some(499),
                None,
                Some("client cancelled".to_string()),
                Some(10),
                None,
                None,
                None,
                982_000,
                Some(982_000),
                Some(983_000),
            )
            .expect("candidate should build"),
            StoredRequestCandidate::new(
                "stale".to_string(),
                "req-stale".to_string(),
                None,
                Some("api-key-1".to_string()),
                None,
                None,
                0,
                0,
                Some("provider-a".to_string()),
                Some("endpoint-a".to_string()),
                Some("key-a".to_string()),
                RequestCandidateStatus::Pending,
                None,
                false,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                699_000,
                Some(699_000),
                None,
            )
            .expect("candidate should build"),
            StoredRequestCandidate::new(
                "other-key".to_string(),
                "req-other-key".to_string(),
                None,
                Some("api-key-1".to_string()),
                None,
                None,
                0,
                0,
                Some("provider-a".to_string()),
                Some("endpoint-a".to_string()),
                Some("key-b".to_string()),
                RequestCandidateStatus::Pending,
                None,
                false,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                990_000,
                Some(990_000),
                None,
            )
            .expect("candidate should build"),
        ];

        assert_eq!(
            count_recent_active_requests_for_provider_key(&recent_candidates, "key-a", 1_000),
            2
        );
    }

    #[test]
    fn fixed_provider_key_rpm_limit_takes_precedence() {
        let key = provider_catalog_key("key-a").with_rate_limit_fields(
            Some(120),
            None,
            Some(80),
            None,
            None,
            None,
            None,
            Some(10),
            Some(10),
        );

        assert_eq!(effective_provider_key_rpm_limit(&key, 100), Some(120));
    }

    #[test]
    fn learned_provider_key_rpm_limit_requires_confidence() {
        let low_confidence = provider_catalog_key("key-a").with_rate_limit_fields(
            None,
            None,
            Some(80),
            Some(0),
            Some(0),
            Some(99),
            None,
            Some(5),
            Some(1),
        );
        assert_eq!(effective_provider_key_rpm_limit(&low_confidence, 100), None);

        let mut high_confidence = provider_catalog_key("key-a").with_rate_limit_fields(
            None,
            None,
            Some(80),
            Some(0),
            Some(0),
            Some(99),
            Some(serde_json::json!([
                {
                    "timestamp": "2026-04-19T00:00:00Z",
                    "old_limit": 0,
                    "new_limit": 80,
                    "reason": "rpm_429",
                    "confidence": 0.8
                },
            ])),
            Some(120),
            Some(118),
        );
        high_confidence.last_429_type = Some("rpm".to_string());
        assert_eq!(
            effective_provider_key_rpm_limit(&high_confidence, 100),
            Some(80)
        );
    }

    #[test]
    fn learned_provider_key_rpm_limit_uses_confirmed_observations_as_fallback_confidence() {
        let key = provider_catalog_key("key-a").with_rate_limit_fields(
            None,
            None,
            Some(80),
            Some(0),
            Some(2),
            Some(99),
            Some(serde_json::json!([
                {
                    "type": "429_observation",
                    "timestamp": "2026-04-19T00:00:00Z",
                    "current_rpm": 90,
                    "upstream_limit": 84
                },
                {
                    "type": "429_observation",
                    "timestamp": "2026-04-19T00:01:00Z",
                    "current_rpm": 88,
                    "upstream_limit": 85
                }
            ])),
            Some(20),
            Some(18),
        );

        assert_eq!(effective_provider_key_rpm_limit(&key, 100), Some(80));
    }

    #[test]
    fn counts_recent_provider_key_rpm_from_snapshot_or_recent_attempts() {
        let recent_candidates = vec![
            StoredRequestCandidate::new(
                "one".to_string(),
                "req-one".to_string(),
                None,
                None,
                None,
                None,
                0,
                0,
                Some("provider-a".to_string()),
                Some("endpoint-a".to_string()),
                Some("key-a".to_string()),
                RequestCandidateStatus::Success,
                None,
                false,
                Some(200),
                None,
                None,
                Some(10),
                Some(7),
                None,
                None,
                95_000,
                Some(95_000),
                Some(96_000),
            )
            .expect("candidate should build"),
            StoredRequestCandidate::new(
                "two".to_string(),
                "req-two".to_string(),
                None,
                None,
                None,
                None,
                0,
                0,
                Some("provider-a".to_string()),
                Some("endpoint-a".to_string()),
                Some("key-a".to_string()),
                RequestCandidateStatus::Failed,
                None,
                false,
                Some(502),
                None,
                None,
                Some(10),
                None,
                None,
                None,
                98_000,
                Some(98_000),
                Some(99_000),
            )
            .expect("candidate should build"),
        ];

        assert_eq!(
            count_recent_rpm_requests_for_provider_key(&recent_candidates, "key-a", 100),
            7
        );
    }

    #[test]
    fn ignores_rpm_observations_before_reset_watermark() {
        let recent_candidates = vec![
            StoredRequestCandidate::new(
                "one".to_string(),
                "req-one".to_string(),
                None,
                None,
                None,
                None,
                0,
                0,
                Some("provider-a".to_string()),
                Some("endpoint-a".to_string()),
                Some("key-a".to_string()),
                RequestCandidateStatus::Success,
                None,
                false,
                Some(200),
                None,
                None,
                Some(10),
                Some(7),
                None,
                None,
                95_000,
                Some(95_000),
                Some(96_000),
            )
            .expect("candidate should build"),
            StoredRequestCandidate::new(
                "two".to_string(),
                "req-two".to_string(),
                None,
                None,
                None,
                None,
                0,
                0,
                Some("provider-a".to_string()),
                Some("endpoint-a".to_string()),
                Some("key-a".to_string()),
                RequestCandidateStatus::Success,
                None,
                false,
                Some(200),
                None,
                None,
                Some(10),
                Some(2),
                None,
                None,
                99_000,
                Some(99_000),
                Some(100_000),
            )
            .expect("candidate should build"),
        ];

        assert_eq!(
            count_recent_rpm_requests_for_provider_key_since(
                &recent_candidates,
                "key-a",
                100,
                Some(98),
            ),
            2
        );
    }

    #[test]
    fn provider_key_rpm_reserves_capacity_for_new_users() {
        let key = provider_catalog_key("key-a").with_rate_limit_fields(
            Some(10),
            None,
            None,
            None,
            None,
            None,
            None,
            Some(5),
            Some(5),
        );
        let recent_candidates = vec![StoredRequestCandidate::new(
            "one".to_string(),
            "req-one".to_string(),
            None,
            None,
            None,
            None,
            0,
            0,
            Some("provider-a".to_string()),
            Some("endpoint-a".to_string()),
            Some("key-a".to_string()),
            RequestCandidateStatus::Success,
            None,
            false,
            Some(200),
            None,
            None,
            Some(10),
            Some(9),
            None,
            None,
            95_000,
            Some(95_000),
            Some(96_000),
        )
        .expect("candidate should build")];

        assert!(!provider_key_rpm_allows_request(
            &key,
            &recent_candidates,
            100,
            false,
        ));
        assert!(provider_key_rpm_allows_request(
            &key,
            &recent_candidates,
            100,
            true,
        ));
        assert!(provider_key_rpm_allows_request_since(
            &key,
            &recent_candidates,
            100,
            false,
            Some(97),
        ));
    }

    #[test]
    fn reads_provider_key_health_and_circuit_status_for_api_format() {
        let key = provider_catalog_key("key-a").with_health_fields(
            Some(serde_json::json!({
                "openai:chat": {"health_score": 0.25},
                "openai:responses": {"health_score": 0.75}
            })),
            Some(serde_json::json!({
                "openai:chat": {"open": true},
                "openai:responses": {"open": false}
            })),
        );

        assert_eq!(provider_key_health_score(&key, "openai:chat"), Some(0.25));
        assert_eq!(
            provider_key_health_score(&key, "openai:responses"),
            Some(0.75)
        );
        assert!(is_provider_key_circuit_open(&key, "openai:chat"));
        assert!(!is_provider_key_circuit_open(&key, "openai:responses"));
    }

    #[test]
    fn aggregates_provider_key_health_score_with_lower_bound_strategy() {
        let key = provider_catalog_key("key-a").with_health_fields(
            Some(serde_json::json!({
                "openai:chat": {"health_score": 0.85},
                "openai:responses": {"health_score": 0.45},
                "claude:messages": {"health_score": 0.70}
            })),
            None,
        );

        assert_eq!(aggregate_provider_key_health_score(&key), Some(0.45));
        assert_eq!(
            effective_provider_key_health_score(&key, "gemini:generate_content"),
            Some(0.45)
        );
    }

    #[test]
    fn classifies_provider_key_health_bucket_from_effective_score() {
        let low = provider_catalog_key("key-low").with_health_fields(
            Some(serde_json::json!({"openai:chat": {"health_score": 0.30}})),
            None,
        );
        let degraded = provider_catalog_key("key-degraded").with_health_fields(
            Some(serde_json::json!({"openai:chat": {"health_score": 0.65}})),
            None,
        );
        let healthy = provider_catalog_key("key-healthy").with_health_fields(
            Some(serde_json::json!({"openai:chat": {"health_score": 0.92}})),
            None,
        );

        assert_eq!(
            provider_key_health_bucket(&low, "openai:chat"),
            Some(ProviderKeyHealthBucket::Low)
        );
        assert_eq!(
            provider_key_health_bucket(&degraded, "openai:chat"),
            Some(ProviderKeyHealthBucket::Degraded)
        );
        assert_eq!(
            provider_key_health_bucket(&healthy, "openai:chat"),
            Some(ProviderKeyHealthBucket::Healthy)
        );
    }
}
