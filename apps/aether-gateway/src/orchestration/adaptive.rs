use std::cmp::Ordering;
use std::collections::BTreeMap;

use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
use chrono::{SecondsFormat, Utc};
use serde_json::{json, Map, Value};

use super::LocalFailoverClassification;
use crate::handlers::shared::default_provider_key_status_snapshot;

const DEFAULT_INITIAL_LIMIT: u32 = 50;
const MIN_RPM_LIMIT: u32 = 5;
const MAX_RPM_LIMIT: u32 = 10_000;
const INCREASE_STEP: u32 = 5;
const UTILIZATION_WINDOW_SIZE: usize = 20;
const UTILIZATION_WINDOW_SECONDS: u64 = 120;
const UTILIZATION_THRESHOLD: f64 = 0.7;
const HIGH_UTILIZATION_RATIO: f64 = 0.6;
const MIN_SAMPLES_FOR_DECISION: usize = 5;
const PROBE_INCREASE_INTERVAL_SECS: u64 = 30 * 60;
const PROBE_INCREASE_MIN_REQUESTS: usize = 10;
const MAX_HISTORY_RECORDS: usize = 20;
const MIN_CONSISTENT_OBSERVATIONS: usize = 3;
const MIN_HEADER_CONFIRMATIONS: usize = 2;
const OBSERVATION_CONSISTENCY_THRESHOLD: f64 = 0.3;
const HEADER_LIMIT_SAFETY_MARGIN: f64 = 0.95;
const OBSERVATION_LIMIT_SAFETY_MARGIN: f64 = 0.90;
const ENFORCEMENT_CONFIDENCE_THRESHOLD: f64 = 0.6;
const CONFIDENCE_DECAY_PER_MINUTE: f64 = 0.005;
const COOLDOWN_AFTER_429_SECS: u64 = 5 * 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LocalAdaptiveRateLimitProjection {
    pub(crate) rpm_429_count: u32,
    pub(crate) last_429_at_unix_secs: u64,
    pub(crate) last_429_type: String,
    pub(crate) learned_rpm_limit: Option<u32>,
    pub(crate) adjustment_history: Option<Value>,
    pub(crate) utilization_samples: Option<Value>,
    pub(crate) last_probe_increase_at_unix_secs: Option<u64>,
    pub(crate) last_rpm_peak: Option<u32>,
    pub(crate) status_snapshot: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LocalAdaptiveSuccessProjection {
    pub(crate) learned_rpm_limit: Option<u32>,
    pub(crate) adjustment_history: Option<Value>,
    pub(crate) utilization_samples: Option<Value>,
    pub(crate) last_probe_increase_at_unix_secs: Option<u64>,
    pub(crate) status_snapshot: Value,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct UtilizationSample {
    ts: u64,
    util: f64,
}

pub(crate) fn project_local_adaptive_rate_limit(
    current_key: &StoredProviderCatalogKey,
    classification: LocalFailoverClassification,
    status_code: u16,
    current_rpm: Option<u32>,
    headers: Option<&BTreeMap<String, String>>,
    observed_at_unix_secs: u64,
) -> Option<LocalAdaptiveRateLimitProjection> {
    if current_key.rpm_limit.is_some() {
        return None;
    }

    if !local_candidate_failure_should_record_adaptive_rate_limit(classification, status_code) {
        return None;
    }

    let upstream_limit =
        parse_latest_upstream_limit(headers).and_then(|value| u32::try_from(value).ok());
    let mut history = adaptive_history_records(current_key.adjustment_history.as_ref());
    let mut rpm_429_count = current_key.rpm_429_count.unwrap_or_default();
    let mut learned_rpm_limit = current_key.learned_rpm_limit;
    let mut last_rpm_peak = current_key.last_rpm_peak;
    let last_probe_increase_at_unix_secs = current_key.last_probe_increase_at_unix_secs;
    let utilization_samples = Some(Value::Array(Vec::new()));

    let last_429_type = "rpm".to_string();
    rpm_429_count = rpm_429_count.saturating_add(1);
    record_429_observation(
        &mut history,
        observed_at_unix_secs,
        current_rpm,
        upstream_limit,
    );

    let (evaluated_limit, confidence) = evaluate_observations(&history);
    if let Some(evaluated_limit) =
        evaluated_limit.filter(|_| confidence >= ENFORCEMENT_CONFIDENCE_THRESHOLD)
    {
        let old_limit = learned_rpm_limit.unwrap_or_default();
        let learning_source = if upstream_limit.is_some() {
            "header"
        } else {
            "observation"
        };
        let mut extra = Map::new();
        extra.insert(
            "current_rpm".to_string(),
            current_rpm.map_or(Value::Null, |value| json!(value)),
        );
        extra.insert(
            "upstream_limit".to_string(),
            upstream_limit.map_or(Value::Null, |value| json!(value)),
        );
        extra.insert("confidence".to_string(), json!(round3(confidence)));
        extra.insert("learning_source".to_string(), json!(learning_source));
        record_adjustment(
            &mut history,
            observed_at_unix_secs,
            old_limit,
            evaluated_limit,
            "rpm_429",
            extra,
        );
        learned_rpm_limit = Some(evaluated_limit);
        last_rpm_peak = upstream_limit.or(current_rpm).or(last_rpm_peak);
    }

    let status_snapshot = project_local_adaptive_status_snapshot(
        current_key,
        &history,
        learned_rpm_limit,
        Some(observed_at_unix_secs),
        last_rpm_peak,
        observed_at_unix_secs,
    );

    Some(LocalAdaptiveRateLimitProjection {
        rpm_429_count,
        last_429_at_unix_secs: observed_at_unix_secs,
        last_429_type,
        learned_rpm_limit,
        adjustment_history: adaptive_history_value(&history),
        utilization_samples,
        last_probe_increase_at_unix_secs,
        last_rpm_peak,
        status_snapshot,
    })
}

pub(crate) fn project_local_adaptive_success(
    current_key: &StoredProviderCatalogKey,
    current_rpm: u32,
    observed_at_unix_secs: u64,
) -> Option<LocalAdaptiveSuccessProjection> {
    if current_key.rpm_limit.is_some() {
        return None;
    }

    let Some(current_limit) = current_key.learned_rpm_limit.filter(|value| *value > 0) else {
        return None;
    };

    let mut history = adaptive_history_records(current_key.adjustment_history.as_ref());
    let confidence = learning_confidence(
        Some(current_limit),
        current_key.last_429_at_unix_secs,
        &history,
        observed_at_unix_secs,
    );
    if confidence < ENFORCEMENT_CONFIDENCE_THRESHOLD {
        return None;
    }

    let known_boundary = current_key.last_rpm_peak;
    let utilization = if current_limit > 0 {
        current_rpm as f64 / current_limit as f64
    } else {
        0.0
    };
    let mut samples = update_utilization_window(
        current_key.utilization_samples.as_ref(),
        observed_at_unix_secs,
        utilization,
    );

    let increase_reason =
        check_increase_conditions(current_key, &samples, observed_at_unix_secs, known_boundary);

    let mut learned_rpm_limit = Some(current_limit);
    let mut last_probe_increase_at_unix_secs = current_key.last_probe_increase_at_unix_secs;
    let utilization_samples = if let Some(reason) = increase_reason {
        let is_probe = reason == "probe_increase";
        let new_limit = increase_limit(current_limit, known_boundary, is_probe);
        if new_limit > current_limit {
            let avg_utilization = average_utilization(&samples);
            let high_util_ratio = high_utilization_ratio(&samples);
            let mut extra = Map::new();
            extra.insert(
                "avg_utilization".to_string(),
                json!(round3(avg_utilization)),
            );
            extra.insert(
                "high_util_ratio".to_string(),
                json!(round3(high_util_ratio)),
            );
            extra.insert("sample_count".to_string(), json!(samples.len()));
            extra.insert("current_rpm".to_string(), json!(current_rpm));
            extra.insert(
                "known_boundary".to_string(),
                known_boundary.map_or(Value::Null, |value| json!(value)),
            );
            extra.insert("confidence".to_string(), json!(round3(confidence)));
            record_adjustment(
                &mut history,
                observed_at_unix_secs,
                current_limit,
                new_limit,
                reason,
                extra,
            );
            learned_rpm_limit = Some(new_limit);
            if is_probe {
                last_probe_increase_at_unix_secs = Some(observed_at_unix_secs);
            }
            samples.clear();
        }
        Value::Array(utilization_sample_values(&samples))
    } else {
        Value::Array(utilization_sample_values(&samples))
    };

    let status_snapshot = project_local_adaptive_status_snapshot(
        current_key,
        &history,
        learned_rpm_limit,
        current_key.last_429_at_unix_secs,
        current_key.last_rpm_peak,
        observed_at_unix_secs,
    );

    Some(LocalAdaptiveSuccessProjection {
        learned_rpm_limit,
        adjustment_history: adaptive_history_value(&history),
        utilization_samples: Some(utilization_samples),
        last_probe_increase_at_unix_secs,
        status_snapshot,
    })
}

fn local_candidate_failure_should_record_adaptive_rate_limit(
    _classification: LocalFailoverClassification,
    status_code: u16,
) -> bool {
    status_code == 429
}

fn project_local_adaptive_status_snapshot(
    current_key: &StoredProviderCatalogKey,
    history: &[Map<String, Value>],
    learned_rpm_limit: Option<u32>,
    last_429_at_unix_secs: Option<u64>,
    last_rpm_peak: Option<u32>,
    observed_at_unix_secs: u64,
) -> Value {
    let default_snapshot = default_provider_key_status_snapshot();
    let mut snapshot = current_key
        .status_snapshot
        .as_ref()
        .and_then(Value::as_object)
        .cloned()
        .or_else(|| default_snapshot.as_object().cloned())
        .unwrap_or_default();

    let observations = history
        .iter()
        .filter(|record| record_type(record) == Some("429_observation"))
        .collect::<Vec<_>>();
    let header_observations = observations
        .iter()
        .filter_map(|record| record_u32(record, "upstream_limit"))
        .collect::<Vec<_>>();
    let learning_confidence = learning_confidence(
        learned_rpm_limit,
        last_429_at_unix_secs,
        history,
        observed_at_unix_secs,
    );

    snapshot.insert("observation_count".to_string(), json!(observations.len()));
    snapshot.insert(
        "header_observation_count".to_string(),
        json!(header_observations.len()),
    );
    snapshot.insert(
        "latest_upstream_limit".to_string(),
        header_observations
            .last()
            .map_or(Value::Null, |value| json!(value)),
    );
    snapshot.insert(
        "learning_confidence".to_string(),
        json!(round3(learning_confidence)),
    );
    snapshot.insert(
        "enforcement_active".to_string(),
        json!(adaptive_enforcement_active(
            learned_rpm_limit,
            learning_confidence,
        )),
    );
    snapshot.insert(
        "known_boundary".to_string(),
        last_rpm_peak.map_or(Value::Null, |value| json!(value)),
    );

    Value::Object(snapshot)
}

fn evaluate_observations(history: &[Map<String, Value>]) -> (Option<u32>, f64) {
    let observations = history
        .iter()
        .filter(|record| record_type(record) == Some("429_observation"))
        .collect::<Vec<_>>();
    if observations.is_empty() {
        return (None, 0.0);
    }

    let header_values = observations
        .iter()
        .filter_map(|record| record_u32(record, "upstream_limit"))
        .collect::<Vec<_>>();
    if header_values.len() >= MIN_HEADER_CONFIRMATIONS {
        let recent = recent_tail(&header_values, MIN_HEADER_CONFIRMATIONS * 2);
        let last_n = recent_tail(recent, MIN_HEADER_CONFIRMATIONS);
        if check_consistency(last_n) {
            let limit = clamp_limit(median(last_n) * HEADER_LIMIT_SAFETY_MARGIN);
            return (Some(limit), 0.8);
        }
    }

    let local_values = observations
        .iter()
        .filter_map(|record| record_u32(record, "current_rpm"))
        .collect::<Vec<_>>();
    if local_values.len() >= MIN_CONSISTENT_OBSERVATIONS {
        let recent = recent_tail(&local_values, MIN_CONSISTENT_OBSERVATIONS * 2);
        let last_n = recent_tail(recent, MIN_CONSISTENT_OBSERVATIONS);
        if check_consistency(last_n) {
            let limit = clamp_limit(median(last_n) * OBSERVATION_LIMIT_SAFETY_MARGIN);
            return (Some(limit), 0.6);
        }
    }

    (None, 0.0)
}

fn learning_confidence(
    learned_rpm_limit: Option<u32>,
    last_429_at_unix_secs: Option<u64>,
    history: &[Map<String, Value>],
    now_unix_secs: u64,
) -> f64 {
    if learned_rpm_limit.is_none() {
        return 0.0;
    }

    let base_confidence = base_learning_confidence(learned_rpm_limit, history);
    if base_confidence <= 0.0 {
        return 0.0;
    }

    let time_decay = match last_429_at_unix_secs {
        Some(last_429_at_unix_secs) => {
            now_unix_secs.saturating_sub(last_429_at_unix_secs) as f64 / 60.0
                * CONFIDENCE_DECAY_PER_MINUTE
        }
        None => 1.0,
    };

    (base_confidence - time_decay).clamp(0.0, 1.0)
}

fn base_learning_confidence(learned_rpm_limit: Option<u32>, history: &[Map<String, Value>]) -> f64 {
    for record in history.iter().rev() {
        if record_type(record) == Some("429_observation") {
            continue;
        }
        if let Some(confidence) = record.get("confidence").and_then(json_value_as_f64) {
            return confidence.clamp(0.0, 1.0);
        }
    }

    let (_, confidence) = evaluate_observations(history);
    if confidence > 0.0 {
        return confidence;
    }

    if learned_rpm_limit.is_some() {
        return 0.3;
    }

    0.0
}

fn adaptive_enforcement_active(learned_rpm_limit: Option<u32>, learning_confidence: f64) -> bool {
    learned_rpm_limit.is_some() && learning_confidence >= ENFORCEMENT_CONFIDENCE_THRESHOLD
}

fn update_utilization_window(
    current_samples: Option<&Value>,
    observed_at_unix_secs: u64,
    utilization: f64,
) -> Vec<UtilizationSample> {
    let mut samples = utilization_samples(current_samples);
    samples.push(UtilizationSample {
        ts: observed_at_unix_secs,
        util: round3(utilization),
    });

    let cutoff_ts = observed_at_unix_secs.saturating_sub(UTILIZATION_WINDOW_SECONDS);
    samples.retain(|sample| sample.ts > cutoff_ts);
    if samples.len() > UTILIZATION_WINDOW_SIZE {
        let keep_from = samples.len() - UTILIZATION_WINDOW_SIZE;
        samples = samples.split_off(keep_from);
    }

    samples
}

fn check_increase_conditions(
    current_key: &StoredProviderCatalogKey,
    samples: &[UtilizationSample],
    observed_at_unix_secs: u64,
    known_boundary: Option<u32>,
) -> Option<&'static str> {
    if is_in_cooldown(current_key.last_429_at_unix_secs, observed_at_unix_secs) {
        return None;
    }

    let current_limit = current_key
        .learned_rpm_limit
        .unwrap_or(DEFAULT_INITIAL_LIMIT);
    if samples.len() >= MIN_SAMPLES_FOR_DECISION {
        let high_util_ratio = high_utilization_ratio(samples);
        if high_util_ratio >= HIGH_UTILIZATION_RATIO {
            if known_boundary.is_none_or(|boundary| current_limit < boundary) {
                return Some("high_utilization");
            }
        }
    }

    should_probe_increase(current_key, samples, observed_at_unix_secs).then_some("probe_increase")
}

fn should_probe_increase(
    current_key: &StoredProviderCatalogKey,
    samples: &[UtilizationSample],
    observed_at_unix_secs: u64,
) -> bool {
    if current_key.last_429_at_unix_secs.is_some_and(|last_429| {
        observed_at_unix_secs.saturating_sub(last_429) < PROBE_INCREASE_INTERVAL_SECS
    }) {
        return false;
    }

    if current_key
        .last_probe_increase_at_unix_secs
        .is_some_and(|last_probe| {
            observed_at_unix_secs.saturating_sub(last_probe) < PROBE_INCREASE_INTERVAL_SECS
        })
    {
        return false;
    }

    if samples.len() < PROBE_INCREASE_MIN_REQUESTS {
        return false;
    }

    average_utilization(samples) >= 0.3
}

fn is_in_cooldown(last_429_at_unix_secs: Option<u64>, now_unix_secs: u64) -> bool {
    last_429_at_unix_secs.is_some_and(|last_429_at| {
        now_unix_secs.saturating_sub(last_429_at) < COOLDOWN_AFTER_429_SECS
    })
}

fn increase_limit(current_limit: u32, known_boundary: Option<u32>, is_probe: bool) -> u32 {
    let mut new_limit = if is_probe {
        current_limit.saturating_add(1)
    } else {
        current_limit.saturating_add(INCREASE_STEP)
    };
    if let Some(known_boundary) = known_boundary.filter(|_| !is_probe) {
        new_limit = new_limit.min(known_boundary);
    }
    new_limit.min(MAX_RPM_LIMIT).max(current_limit)
}

fn record_429_observation(
    history: &mut Vec<Map<String, Value>>,
    observed_at_unix_secs: u64,
    current_rpm: Option<u32>,
    upstream_limit: Option<u32>,
) {
    let mut record = Map::new();
    record.insert("type".to_string(), json!("429_observation"));
    record.insert(
        "timestamp".to_string(),
        json!(timestamp_string(observed_at_unix_secs)),
    );
    record.insert(
        "current_rpm".to_string(),
        current_rpm.map_or(Value::Null, |value| json!(value)),
    );
    record.insert(
        "upstream_limit".to_string(),
        upstream_limit.map_or(Value::Null, |value| json!(value)),
    );
    history.push(record);
    trim_history(history);
}

fn record_adjustment(
    history: &mut Vec<Map<String, Value>>,
    observed_at_unix_secs: u64,
    old_limit: u32,
    new_limit: u32,
    reason: &str,
    extra: Map<String, Value>,
) {
    let mut record = Map::new();
    record.insert(
        "timestamp".to_string(),
        json!(timestamp_string(observed_at_unix_secs)),
    );
    record.insert("old_limit".to_string(), json!(old_limit));
    record.insert("new_limit".to_string(), json!(new_limit));
    record.insert("reason".to_string(), json!(reason));
    record.extend(extra);
    history.push(record);
    trim_history(history);
}

fn trim_history(history: &mut Vec<Map<String, Value>>) {
    if history.len() <= MAX_HISTORY_RECORDS {
        return;
    }

    let mut observations = history
        .iter()
        .filter(|record| record_type(record) == Some("429_observation"))
        .cloned()
        .collect::<Vec<_>>();
    let mut adjustments = history
        .iter()
        .filter(|record| record_type(record) != Some("429_observation"))
        .cloned()
        .collect::<Vec<_>>();

    sort_history_by_timestamp(&mut observations);
    sort_history_by_timestamp(&mut adjustments);

    let mut overflow = history.len() - MAX_HISTORY_RECORDS;
    let trim_adjustments = overflow.min(adjustments.len());
    adjustments.drain(0..trim_adjustments);
    overflow -= trim_adjustments;

    if overflow > 0 {
        observations.drain(0..overflow.min(observations.len()));
    }

    history.clear();
    history.extend(observations);
    history.extend(adjustments);
    sort_history_by_timestamp(history);
}

fn sort_history_by_timestamp(history: &mut [Map<String, Value>]) {
    history.sort_by(|left, right| {
        left.get("timestamp")
            .and_then(Value::as_str)
            .cmp(&right.get("timestamp").and_then(Value::as_str))
    });
}

fn adaptive_history_records(value: Option<&Value>) -> Vec<Map<String, Value>> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .cloned()
        .collect()
}

fn adaptive_history_value(history: &[Map<String, Value>]) -> Option<Value> {
    (!history.is_empty()).then(|| {
        Value::Array(
            history
                .iter()
                .cloned()
                .map(Value::Object)
                .collect::<Vec<_>>(),
        )
    })
}

fn utilization_samples(value: Option<&Value>) -> Vec<UtilizationSample> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter_map(|record| {
            Some(UtilizationSample {
                ts: record_u64(record, "ts")?,
                util: record
                    .get("util")
                    .and_then(json_value_as_f64)?
                    .clamp(0.0, 10.0),
            })
        })
        .collect()
}

fn utilization_sample_values(samples: &[UtilizationSample]) -> Vec<Value> {
    samples
        .iter()
        .map(|sample| {
            json!({
                "ts": sample.ts,
                "util": round3(sample.util),
            })
        })
        .collect()
}

fn average_utilization(samples: &[UtilizationSample]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }

    samples.iter().map(|sample| sample.util).sum::<f64>() / samples.len() as f64
}

fn high_utilization_ratio(samples: &[UtilizationSample]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }

    samples
        .iter()
        .filter(|sample| sample.util >= UTILIZATION_THRESHOLD)
        .count() as f64
        / samples.len() as f64
}

fn check_consistency(values: &[u32]) -> bool {
    let med = median(values);
    med > 0.0
        && values
            .iter()
            .all(|value| (*value as f64 - med).abs() / med <= OBSERVATION_CONSISTENCY_THRESHOLD)
}

fn median(values: &[u32]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }

    let mut sorted = values.iter().map(|value| *value as f64).collect::<Vec<_>>();
    sorted.sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
    let midpoint = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        (sorted[midpoint - 1] + sorted[midpoint]) / 2.0
    } else {
        sorted[midpoint]
    }
}

fn clamp_limit(value: f64) -> u32 {
    value
        .floor()
        .clamp(MIN_RPM_LIMIT as f64, MAX_RPM_LIMIT as f64) as u32
}

fn record_type(record: &Map<String, Value>) -> Option<&str> {
    record.get("type").and_then(Value::as_str)
}

fn record_u32(record: &Map<String, Value>, field: &str) -> Option<u32> {
    record
        .get(field)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
}

fn record_u64(record: &Map<String, Value>, field: &str) -> Option<u64> {
    record.get(field).and_then(Value::as_u64)
}

fn json_value_as_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|raw| raw as f64))
        .or_else(|| value.as_u64().map(|raw| raw as f64))
}

fn recent_tail<T>(values: &[T], limit: usize) -> &[T] {
    let keep_from = values.len().saturating_sub(limit);
    &values[keep_from..]
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

fn timestamp_string(unix_secs: u64) -> String {
    chrono::DateTime::<Utc>::from_timestamp(unix_secs as i64, 0)
        .map(|value| value.to_rfc3339_opts(SecondsFormat::Secs, true))
        .unwrap_or_else(|| Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true))
}

fn parse_latest_upstream_limit(headers: Option<&BTreeMap<String, String>>) -> Option<u64> {
    let normalized = headers?
        .iter()
        .map(|(key, value)| (key.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect::<BTreeMap<_, _>>();

    const CANDIDATE_KEYS: &[&str] = &[
        "x-ratelimit-limit-requests",
        "x-ratelimit-limit-request",
        "x-ratelimit-limit",
        "x-rate-limit-limit",
        "ratelimit-limit",
    ];

    for key in CANDIDATE_KEYS {
        if let Some(limit) = normalized
            .get(*key)
            .and_then(|value| parse_limit_header_value(value))
        {
            return Some(limit);
        }
    }

    normalized.iter().find_map(|(key, value)| {
        if !key.contains("ratelimit") || !key.contains("limit") {
            return None;
        }
        if key.contains("token") {
            return None;
        }
        parse_limit_header_value(value)
    })
}

fn parse_limit_header_value(raw: &str) -> Option<u64> {
    raw.split([',', ';'])
        .find_map(|part| {
            let digits = part
                .trim()
                .chars()
                .take_while(|ch| ch.is_ascii_digit())
                .collect::<String>();
            (!digits.is_empty())
                .then(|| digits.parse::<u64>().ok())
                .flatten()
        })
        .filter(|value| *value > 0)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        project_local_adaptive_rate_limit, project_local_adaptive_success,
        LocalAdaptiveRateLimitProjection,
    };
    use crate::orchestration::LocalFailoverClassification;
    use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
    use serde_json::{json, Value};

    fn sample_adaptive_key() -> StoredProviderCatalogKey {
        let mut key = StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "adaptive".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build");
        key.rpm_limit = None;
        key
    }

    fn assert_rpm_projection(projection: &LocalAdaptiveRateLimitProjection) {
        assert_eq!(projection.last_429_type, "rpm");
        assert_eq!(projection.utilization_samples, Some(json!([])));
        assert!(projection.adjustment_history.is_some());
    }

    #[test]
    fn rate_limit_projection_increments_adaptive_rpm_observation() {
        let mut key = sample_adaptive_key();
        key.rpm_429_count = Some(2);

        let projection = project_local_adaptive_rate_limit(
            &key,
            LocalFailoverClassification::RetryUpstreamFailure,
            429,
            Some(19),
            None,
            1_760_000_000,
        )
        .expect("projection should exist");

        assert_rpm_projection(&projection);
        assert_eq!(projection.rpm_429_count, 3);
        assert_eq!(projection.last_429_at_unix_secs, 1_760_000_000);
        assert_eq!(projection.learned_rpm_limit, None);
        assert_eq!(projection.status_snapshot["observation_count"], json!(1));
        assert_eq!(
            projection.status_snapshot["learning_confidence"],
            json!(0.0)
        );
        assert_eq!(
            projection.status_snapshot["enforcement_active"],
            json!(false)
        );
    }

    #[test]
    fn rate_limit_projection_ignores_fixed_limit_keys() {
        let mut key = sample_adaptive_key();
        key.rpm_limit = Some(20);

        assert!(project_local_adaptive_rate_limit(
            &key,
            LocalFailoverClassification::RetryUpstreamFailure,
            429,
            Some(10),
            None,
            1_760_000_000,
        )
        .is_none());
    }

    #[test]
    fn rate_limit_projection_ignores_non_rate_limit_failures() {
        let key = sample_adaptive_key();

        assert!(project_local_adaptive_rate_limit(
            &key,
            LocalFailoverClassification::RetryUpstreamFailure,
            503,
            Some(10),
            None,
            1_760_000_000,
        )
        .is_none());
    }

    #[test]
    fn rate_limit_projection_learns_limit_from_consistent_headers() {
        let mut key = sample_adaptive_key();
        key.adjustment_history = Some(json!([
            {
                "type": "429_observation",
                "timestamp": "2026-04-19T00:00:00Z",
                "current_rpm": 44,
                "upstream_limit": 40
            }
        ]));
        let headers =
            BTreeMap::from([("x-ratelimit-limit-requests".to_string(), "42".to_string())]);

        let projection = project_local_adaptive_rate_limit(
            &key,
            LocalFailoverClassification::RetryUpstreamFailure,
            429,
            Some(45),
            Some(&headers),
            1_760_000_000,
        )
        .expect("projection should exist");

        assert_rpm_projection(&projection);
        assert_eq!(projection.rpm_429_count, 1);
        assert_eq!(projection.learned_rpm_limit, Some(38));
        assert_eq!(projection.last_rpm_peak, Some(42));
        assert_eq!(
            projection.status_snapshot["header_observation_count"],
            json!(2)
        );
        assert_eq!(
            projection.status_snapshot["latest_upstream_limit"],
            json!(42)
        );
        assert_eq!(
            projection.status_snapshot["learning_confidence"],
            json!(0.8)
        );
        assert_eq!(
            projection.status_snapshot["enforcement_active"],
            json!(true)
        );
        assert_eq!(
            projection
                .adjustment_history
                .as_ref()
                .and_then(Value::as_array)
                .map(|items| items.len()),
            Some(3)
        );
    }

    #[test]
    fn rate_limit_projection_learns_limit_from_consistent_local_observations() {
        let mut key = sample_adaptive_key();
        key.adjustment_history = Some(json!([
            {
                "type": "429_observation",
                "timestamp": "2026-04-19T00:00:00Z",
                "current_rpm": 20,
                "upstream_limit": null
            },
            {
                "type": "429_observation",
                "timestamp": "2026-04-19T00:01:00Z",
                "current_rpm": 22,
                "upstream_limit": null
            }
        ]));

        let projection = project_local_adaptive_rate_limit(
            &key,
            LocalFailoverClassification::RetryUpstreamFailure,
            429,
            Some(21),
            None,
            1_760_000_000,
        )
        .expect("projection should exist");

        assert_eq!(projection.learned_rpm_limit, Some(18));
        assert_eq!(projection.last_rpm_peak, Some(21));
        assert_eq!(
            projection.status_snapshot["learning_confidence"],
            json!(0.6)
        );
        assert_eq!(
            projection.status_snapshot["enforcement_active"],
            json!(true)
        );
    }

    #[test]
    fn rate_limit_projection_records_429_as_rpm_observation() {
        let mut key = sample_adaptive_key();
        key.learned_rpm_limit = Some(100);

        let projection = project_local_adaptive_rate_limit(
            &key,
            LocalFailoverClassification::RetryUpstreamFailure,
            429,
            Some(110),
            None,
            1_760_000_000,
        )
        .expect("projection should exist");

        assert_eq!(projection.last_429_type, "rpm");
        assert_eq!(projection.rpm_429_count, 1);
        assert_eq!(projection.learned_rpm_limit, Some(100));
        assert_eq!(
            projection
                .adjustment_history
                .as_ref()
                .and_then(Value::as_array)
                .map(|items| items.len()),
            Some(1)
        );
    }

    #[test]
    fn adaptive_success_projection_expands_limit_for_high_utilization() {
        let mut key = sample_adaptive_key();
        key.learned_rpm_limit = Some(20);
        key.last_rpm_peak = Some(25);
        key.last_429_at_unix_secs = Some(1_759_999_000);
        key.adjustment_history = Some(json!([
            {
                "timestamp": "2026-04-19T00:00:00Z",
                "old_limit": 0,
                "new_limit": 20,
                "reason": "rpm_429",
                "confidence": 0.8
            }
        ]));
        key.utilization_samples = Some(json!([
            {"ts": 1759999960, "util": 0.90},
            {"ts": 1759999970, "util": 0.95},
            {"ts": 1759999980, "util": 0.85},
            {"ts": 1759999990, "util": 0.80}
        ]));

        let projection = project_local_adaptive_success(&key, 19, 1_760_000_000)
            .expect("projection should exist");

        assert_eq!(projection.learned_rpm_limit, Some(25));
        assert_eq!(projection.utilization_samples, Some(json!([])));
        assert_eq!(
            projection
                .adjustment_history
                .as_ref()
                .and_then(Value::as_array)
                .and_then(|items| items.last())
                .and_then(Value::as_object)
                .and_then(|record| record.get("reason"))
                .and_then(Value::as_str),
            Some("high_utilization")
        );
    }

    #[test]
    fn adaptive_success_projection_ignores_low_confidence_learning() {
        let mut key = sample_adaptive_key();
        key.learned_rpm_limit = Some(20);
        key.adjustment_history = Some(json!([
            {
                "timestamp": "2026-04-19T00:00:00Z",
                "old_limit": 0,
                "new_limit": 20,
                "reason": "rpm_429",
                "confidence": 0.55
            }
        ]));
        key.last_429_at_unix_secs = Some(1_760_000_000);

        assert!(project_local_adaptive_success(&key, 18, 1_760_000_000).is_none());
    }
}
