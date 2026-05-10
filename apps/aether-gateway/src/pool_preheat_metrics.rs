use std::collections::BTreeMap;
use std::sync::Mutex;

use aether_runtime::{MetricKind, MetricLabel, MetricSample};

const PROBE_RUNS_TOTAL: &str = "aether_pool_preheat_probe_runs_total";
const PROBE_OUTCOMES_TOTAL: &str = "aether_pool_preheat_probe_outcomes_total";
const DEDUP_SKIPPED_TOTAL: &str = "aether_pool_preheat_dedup_skipped_total";
const RATE_LIMIT_REJECTED_TOTAL: &str = "aether_pool_preheat_rate_limit_rejected_total";
const CIRCUIT_SUSPENDED_TOTAL: &str = "aether_pool_preheat_circuit_suspended_total";
const CANDIDATE_CACHE_OPERATIONS_TOTAL: &str = "aether_pool_candidate_cache_operations_total";
const HEDGE_SWAP_TOTAL: &str = "aether_hedge_swap_total";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CounterLabel {
    key: &'static str,
    value: String,
}

impl CounterLabel {
    fn new(key: &'static str, value: &str) -> Self {
        Self {
            key,
            value: metric_label_value(value, "unknown"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CounterKey {
    name: &'static str,
    labels: Vec<CounterLabel>,
}

#[derive(Debug, Default)]
pub(crate) struct PoolPreheatMetrics {
    counters: Mutex<BTreeMap<CounterKey, u64>>,
}

impl PoolPreheatMetrics {
    pub(crate) fn record_probe_run(&self, trigger: &str, outcome: &str) {
        self.increment(
            PROBE_RUNS_TOTAL,
            vec![
                CounterLabel::new("trigger", trigger),
                CounterLabel::new("outcome", outcome),
            ],
            1,
        );
    }

    pub(crate) fn record_probe_outcome(&self, provider_type: &str, outcome_kind: &str) {
        self.increment(
            PROBE_OUTCOMES_TOTAL,
            vec![
                CounterLabel::new("provider_type", provider_type),
                CounterLabel::new("outcome_kind", outcome_kind),
            ],
            1,
        );
    }

    pub(crate) fn record_dedup_skipped(&self, provider_type: &str, count: u64) {
        self.increment(
            DEDUP_SKIPPED_TOTAL,
            vec![CounterLabel::new("provider_type", provider_type)],
            count,
        );
    }

    pub(crate) fn record_rate_limit_rejected(&self, provider_type: &str) {
        self.increment(
            RATE_LIMIT_REJECTED_TOTAL,
            vec![CounterLabel::new("provider_type", provider_type)],
            1,
        );
    }

    pub(crate) fn record_circuit_suspended(&self, provider_type: &str) {
        self.increment(
            CIRCUIT_SUSPENDED_TOTAL,
            vec![CounterLabel::new("provider_type", provider_type)],
            1,
        );
    }

    pub(crate) fn record_candidate_cache_operation(&self, operation: &str) {
        self.increment(
            CANDIDATE_CACHE_OPERATIONS_TOTAL,
            vec![CounterLabel::new("operation", operation)],
            1,
        );
    }

    pub(crate) fn record_hedge_swap(&self, trigger_error_kind: &str) {
        self.increment(
            HEDGE_SWAP_TOTAL,
            vec![CounterLabel::new("trigger_error_kind", trigger_error_kind)],
            1,
        );
    }

    pub(crate) fn metric_samples(&self) -> Vec<MetricSample> {
        self.counters
            .lock()
            .expect("pool preheat metrics lock poisoned")
            .iter()
            .map(|(key, value)| {
                MetricSample::new(key.name, metric_help(key.name), MetricKind::Counter, *value)
                    .with_labels(
                        key.labels
                            .iter()
                            .map(|label| MetricLabel::new(label.key, label.value.clone()))
                            .collect(),
                    )
            })
            .collect()
    }

    fn increment(&self, name: &'static str, labels: Vec<CounterLabel>, value: u64) {
        if value == 0 {
            return;
        }
        let key = CounterKey { name, labels };
        let mut counters = self
            .counters
            .lock()
            .expect("pool preheat metrics lock poisoned");
        let counter = counters.entry(key).or_default();
        *counter = counter.saturating_add(value);
    }
}

fn metric_label_value(value: &str, fallback: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        fallback.to_string()
    } else {
        value.chars().take(80).collect()
    }
}

fn metric_help(name: &str) -> &'static str {
    match name {
        PROBE_RUNS_TOTAL => "Number of OAuth pool preheat probe runs by trigger and outcome.",
        PROBE_OUTCOMES_TOTAL => "Number of OAuth pool preheat probe key outcomes by provider type.",
        DEDUP_SKIPPED_TOTAL => {
            "Number of OAuth pool preheat probe keys skipped by recent healthy dedup stamps."
        }
        RATE_LIMIT_REJECTED_TOTAL => {
            "Number of OAuth pool preheat probe runs rejected by provider type rate limits."
        }
        CIRCUIT_SUSPENDED_TOTAL => {
            "Number of OAuth pool preheat probe runs suspended by provider type circuit state."
        }
        CANDIDATE_CACHE_OPERATIONS_TOTAL => {
            "Number of OAuth pool candidate cache operations observed by preheat paths."
        }
        HEDGE_SWAP_TOTAL => {
            "Number of hedge swaps that promoted preheated pool candidates by trigger error kind."
        }
        _ => "Gateway OAuth pool preheat counter.",
    }
}

#[cfg(test)]
mod tests {
    use super::PoolPreheatMetrics;

    #[test]
    fn records_labeled_counter_samples() {
        let metrics = PoolPreheatMetrics::default();

        metrics.record_probe_run("candidate_loop", "completed");
        metrics.record_probe_run("candidate_loop", "completed");
        metrics.record_candidate_cache_operation("hit");

        let samples = metrics.metric_samples();
        let run_sample = samples
            .iter()
            .find(|sample| sample.name == "aether_pool_preheat_probe_runs_total")
            .expect("probe run sample should exist");
        assert_eq!(run_sample.value, 2);
        assert!(run_sample
            .labels
            .iter()
            .any(|label| label.key == "trigger" && label.value == "candidate_loop"));
        assert!(run_sample
            .labels
            .iter()
            .any(|label| label.key == "outcome" && label.value == "completed"));

        let cache_sample = samples
            .iter()
            .find(|sample| sample.name == "aether_pool_candidate_cache_operations_total")
            .expect("cache operation sample should exist");
        assert_eq!(cache_sample.value, 1);
    }
}
