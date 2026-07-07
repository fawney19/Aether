use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aether_contracts::ExecutionPlan;
use aether_runtime::{
    ConcurrencyError, ConcurrencyGate, ConcurrencyPermit, MetricKind, MetricLabel, MetricSample,
};
use dashmap::DashMap;
use tokio::time::timeout;
use url::Url;

use crate::stage_metrics::observe_gateway_stage_ms;
use crate::GatewayError;

const GATE_NAME: &str = "gateway_upstream_target";
const DEFAULT_METRIC_TARGET_LIMIT: usize = 32;
const METRIC_TARGET_LIMIT_ENV: &str = "AETHER_GATEWAY_UPSTREAM_TARGET_GATE_METRIC_LIMIT";
const TARGET_QUEUE_BUDGET_MS_ENV: &str = "AETHER_GATEWAY_UPSTREAM_TARGET_GATE_QUEUE_BUDGET_MS";
const DEFAULT_TARGET_QUEUE_BUDGET_MS: u64 = 1;
const MAX_TARGET_QUEUE_BUDGET_MS: u64 = 5_000;

#[derive(Debug)]
pub(crate) struct UpstreamTargetAdmission {
    limit: Option<usize>,
    queue_budget: Duration,
    gates: DashMap<String, Arc<UpstreamTargetGate>>,
}

#[derive(Debug)]
pub(crate) struct UpstreamTargetAdmissionPermit {
    _permit: ConcurrencyPermit,
}

#[derive(Debug)]
struct UpstreamTargetGate {
    gate: ConcurrencyGate,
    raw_seen_total: AtomicU64,
    preselect_total: AtomicU64,
    selected_total: AtomicU64,
    saturated_total: AtomicU64,
}

impl UpstreamTargetGate {
    fn new(limit: usize) -> Self {
        Self {
            gate: ConcurrencyGate::new(GATE_NAME, limit),
            raw_seen_total: AtomicU64::new(0),
            preselect_total: AtomicU64::new(0),
            selected_total: AtomicU64::new(0),
            saturated_total: AtomicU64::new(0),
        }
    }

    fn raw_seen(&self) {
        self.raw_seen_total.fetch_add(1, Ordering::Relaxed);
    }

    fn preselected(&self) {
        self.preselect_total.fetch_add(1, Ordering::Relaxed);
    }

    fn selected(&self) {
        self.selected_total.fetch_add(1, Ordering::Relaxed);
    }

    fn saturated(&self) {
        self.saturated_total.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Debug, Clone)]
pub(crate) struct UpstreamTargetAdmissionSnapshot {
    pub(crate) target: String,
    pub(crate) in_flight: usize,
    pub(crate) available_permits: usize,
    pub(crate) high_watermark: usize,
    pub(crate) rejected: u64,
    pub(crate) raw_seen_total: u64,
    pub(crate) preselect_total: u64,
    pub(crate) selected_total: u64,
    pub(crate) selection_pressure_total: u64,
    pub(crate) saturated_total: u64,
}

impl UpstreamTargetAdmission {
    pub(crate) fn new(limit: Option<usize>, queue_budget: Duration) -> Self {
        Self {
            limit,
            queue_budget: target_queue_budget(queue_budget),
            gates: DashMap::new(),
        }
    }

    pub(crate) async fn acquire(
        &self,
        plan: &ExecutionPlan,
        trace_id: &str,
    ) -> Result<Option<UpstreamTargetAdmissionPermit>, GatewayError> {
        let Some(limit) = self.limit else {
            return Ok(None);
        };
        let key = upstream_target_key(plan);
        let gate = self
            .gates
            .entry(key.clone())
            .or_insert_with(|| Arc::new(UpstreamTargetGate::new(limit)))
            .clone();
        gate.selected();
        let started_at = Instant::now();
        let permit = match timeout(self.queue_budget, gate.gate.acquire()).await {
            Ok(Ok(permit)) => permit,
            Ok(Err(err)) => return Err(GatewayError::Internal(err.to_string())),
            Err(_) => {
                gate.saturated();
                tracing::debug!(
                    event_name = "gateway_upstream_target_admission_timeout",
                    log_type = "ops",
                    trace_id,
                    target = key.as_str(),
                    limit,
                    queue_budget_ms = self.queue_budget.as_millis() as u64,
                    "gateway upstream target admission gate timed out"
                );
                return Err(GatewayError::AdmissionTimeout {
                    trace_id: trace_id.to_string(),
                    gate: GATE_NAME,
                    queue_budget_ms: self.queue_budget.as_millis() as u64,
                });
            }
        };
        observe_gateway_stage_ms(
            "stream_upstream_target_admission",
            started_at.elapsed().as_millis() as u64,
        );
        Ok(Some(UpstreamTargetAdmissionPermit { _permit: permit }))
    }

    pub(crate) fn try_acquire_for_plan(
        &self,
        plan: &ExecutionPlan,
    ) -> Result<Option<UpstreamTargetAdmissionPermit>, GatewayError> {
        let Some(limit) = self.limit else {
            return Ok(None);
        };
        let key = upstream_target_key(plan);
        let gate = self
            .gates
            .entry(key)
            .or_insert_with(|| Arc::new(UpstreamTargetGate::new(limit)))
            .clone();
        gate.selected();
        match gate.gate.try_acquire() {
            Ok(permit) => Ok(Some(UpstreamTargetAdmissionPermit { _permit: permit })),
            Err(ConcurrencyError::Saturated { .. }) => {
                gate.saturated();
                Ok(None)
            }
            Err(err) => Err(GatewayError::Internal(err.to_string())),
        }
    }

    pub(crate) fn snapshot_for_plan(
        &self,
        plan: &ExecutionPlan,
    ) -> Option<UpstreamTargetAdmissionSnapshot> {
        let key = upstream_target_key(plan);
        self.snapshot_for_target_key(&key)
    }

    pub(crate) fn snapshot_for_target_key(
        &self,
        target: &str,
    ) -> Option<UpstreamTargetAdmissionSnapshot> {
        let entry = self.gates.get(target)?;
        Some(snapshot_for_gate(target.to_string(), entry.value()))
    }

    pub(crate) fn record_preselect_for_target_key(&self, target: &str) {
        let Some(limit) = self.limit else {
            return;
        };
        let gate = self
            .gates
            .entry(target.to_string())
            .or_insert_with(|| Arc::new(UpstreamTargetGate::new(limit)));
        gate.preselected();
    }

    pub(crate) fn record_raw_seen_for_target_key(&self, target: &str) {
        let Some(limit) = self.limit else {
            return;
        };
        let gate = self
            .gates
            .entry(target.to_string())
            .or_insert_with(|| Arc::new(UpstreamTargetGate::new(limit)));
        gate.raw_seen();
    }

    pub(crate) fn limit(&self) -> Option<usize> {
        self.limit
    }

    pub(crate) fn metric_samples(&self) -> Vec<MetricSample> {
        let mut samples = vec![MetricSample::new(
            "upstream_target_gate_active_targets",
            "Number of upstream targets currently tracked by the gateway upstream target admission gates.",
            MetricKind::Gauge,
            self.gates.len() as u64,
        )];

        let Some(limit) = self.limit else {
            return samples;
        };

        samples.push(MetricSample::new(
            "upstream_target_gate_limit",
            "Configured per-upstream-target admission gate limit.",
            MetricKind::Gauge,
            limit as u64,
        ));

        let mut snapshots = self
            .gates
            .iter()
            .map(|entry| snapshot_for_gate(entry.key().clone(), entry.value()))
            .collect::<Vec<_>>();
        snapshots.sort_by(|left, right| {
            right
                .in_flight
                .cmp(&left.in_flight)
                .then_with(|| right.high_watermark.cmp(&left.high_watermark))
                .then_with(|| right.saturated_total.cmp(&left.saturated_total))
        });

        let metric_target_limit = upstream_target_metric_limit();
        for snapshot in snapshots.into_iter().take(metric_target_limit) {
            let labels = vec![MetricLabel::new("target", snapshot.target)];
            samples.push(
                MetricSample::new(
                    "upstream_target_gate_in_flight",
                    "Current number of in-flight operations for an upstream target admission gate.",
                    MetricKind::Gauge,
                    snapshot.in_flight as u64,
                )
                .with_labels(labels.clone()),
            );
            samples.push(
                MetricSample::new(
                    "upstream_target_gate_available_permits",
                    "Currently available permits for an upstream target admission gate.",
                    MetricKind::Gauge,
                    snapshot.available_permits as u64,
                )
                .with_labels(labels.clone()),
            );
            samples.push(
                MetricSample::new(
                    "upstream_target_gate_high_watermark",
                    "Highest observed in-flight count for an upstream target admission gate.",
                    MetricKind::Gauge,
                    snapshot.high_watermark as u64,
                )
                .with_labels(labels.clone()),
            );
            samples.push(
                MetricSample::new(
                    "upstream_target_gate_rejected_total",
                    "Number of operations rejected by an upstream target admission gate.",
                    MetricKind::Counter,
                    snapshot.rejected,
                )
                .with_labels(labels.clone()),
            );
            samples.push(
                MetricSample::new(
                    "upstream_target_selected_total",
                    "Number of selections for an upstream target.",
                    MetricKind::Counter,
                    snapshot.selected_total,
                )
                .with_labels(labels.clone()),
            );
            samples.push(
                MetricSample::new(
                    "upstream_target_raw_seen_total",
                    "Number of lightweight target-selection windows where an upstream target appeared.",
                    MetricKind::Counter,
                    snapshot.raw_seen_total,
                )
                .with_labels(labels.clone()),
            );
            samples.push(
                MetricSample::new(
                    "upstream_target_preselect_total",
                    "Number of lightweight pre-first-byte selections for an upstream target.",
                    MetricKind::Counter,
                    snapshot.preselect_total,
                )
                .with_labels(labels.clone()),
            );
            samples.push(
                MetricSample::new(
                    "upstream_target_in_flight",
                    "Current number of pre-first-byte in-flight operations for an upstream target.",
                    MetricKind::Gauge,
                    snapshot.in_flight as u64,
                )
                .with_labels(labels.clone()),
            );
            samples.push(
                MetricSample::new(
                    "upstream_target_max_in_flight",
                    "Highest observed pre-first-byte in-flight count for an upstream target.",
                    MetricKind::Gauge,
                    snapshot.high_watermark as u64,
                )
                .with_labels(labels.clone()),
            );
            samples.push(
                MetricSample::new(
                    "upstream_target_saturated_total",
                    "Number of saturated selections for an upstream target.",
                    MetricKind::Counter,
                    snapshot.saturated_total,
                )
                .with_labels(labels),
            );
        }

        samples
    }
}

fn snapshot_for_gate(target: String, gate: &UpstreamTargetGate) -> UpstreamTargetAdmissionSnapshot {
    let snapshot = gate.gate.snapshot();
    let raw_seen_total = gate.raw_seen_total.load(Ordering::Relaxed);
    let preselect_total = gate.preselect_total.load(Ordering::Relaxed);
    let selected_total = gate.selected_total.load(Ordering::Relaxed);
    UpstreamTargetAdmissionSnapshot {
        target,
        in_flight: snapshot.in_flight,
        available_permits: snapshot.available_permits,
        high_watermark: snapshot.high_watermark,
        rejected: snapshot.rejected,
        raw_seen_total,
        preselect_total,
        selected_total,
        selection_pressure_total: preselect_total.saturating_add(selected_total),
        saturated_total: gate.saturated_total.load(Ordering::Relaxed),
    }
}

pub(crate) fn upstream_target_key(plan: &ExecutionPlan) -> String {
    let proxy = plan
        .proxy
        .as_ref()
        .and_then(|proxy| proxy.url.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    upstream_target_key_from_url(plan.url.as_str(), proxy)
        .unwrap_or_else(|| fallback_target_key(plan))
}

pub(crate) fn upstream_target_key_from_url(
    upstream_url: &str,
    proxy: Option<&str>,
) -> Option<String> {
    let parsed = Url::parse(upstream_url).ok();
    let Some(url) = parsed else {
        return None;
    };
    let scheme = url.scheme().to_ascii_lowercase();
    let Some(host) = url.host_str().map(|host| host.to_ascii_lowercase()) else {
        return None;
    };
    let port = url
        .port_or_known_default()
        .map(|port| port.to_string())
        .unwrap_or_else(|| "-".to_string());
    let proxy = proxy
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("-");
    Some(format!("{scheme}://{host}:{port}|proxy={proxy}"))
}

fn fallback_target_key(plan: &ExecutionPlan) -> String {
    format!(
        "unparsed|provider={}|endpoint={}|url={}",
        plan.provider_id, plan.endpoint_id, plan.url
    )
}

fn upstream_target_metric_limit() -> usize {
    std::env::var(METRIC_TARGET_LIMIT_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(DEFAULT_METRIC_TARGET_LIMIT)
}

fn target_queue_budget(fallback: Duration) -> Duration {
    std::env::var(TARGET_QUEUE_BUDGET_MS_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|value| value.clamp(1, MAX_TARGET_QUEUE_BUDGET_MS))
        .map(Duration::from_millis)
        .unwrap_or_else(|| {
            let fallback_ms = u64::try_from(fallback.as_millis()).unwrap_or(u64::MAX);
            Duration::from_millis(fallback_ms.clamp(1, DEFAULT_TARGET_QUEUE_BUDGET_MS))
        })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use aether_contracts::{ExecutionPlan, RequestBody};
    use serde_json::json;

    use super::*;

    fn test_plan(url: &str) -> ExecutionPlan {
        ExecutionPlan {
            request_id: "req-upstream-target".to_string(),
            candidate_id: Some("cand-upstream-target".to_string()),
            provider_name: Some("provider".to_string()),
            provider_id: "provider_id".to_string(),
            endpoint_id: "endpoint_id".to_string(),
            key_id: "key_id".to_string(),
            method: "POST".to_string(),
            url: url.to_string(),
            headers: Default::default(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"stream": true})),
            stream: true,
            client_api_format: "openai".to_string(),
            provider_api_format: "openai".to_string(),
            model_name: Some("model".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        }
    }

    #[test]
    fn upstream_target_key_ignores_path_and_query() {
        let left = test_plan("http://127.0.0.1:18181/v1/chat/completions?x=1");
        let right = test_plan("http://127.0.0.1:18181/v1/responses");

        assert_eq!(upstream_target_key(&left), upstream_target_key(&right));
    }

    #[test]
    fn upstream_target_key_from_url_matches_plan_key_without_proxy() {
        let plan = test_plan("http://127.0.0.1:18181/v1/chat/completions?x=1");

        assert_eq!(
            upstream_target_key_from_url("http://127.0.0.1:18181/v1/responses", None)
                .expect("url should parse"),
            upstream_target_key(&plan)
        );
    }

    #[test]
    fn upstream_target_key_from_url_includes_proxy() {
        assert_eq!(
            upstream_target_key_from_url(
                "https://api.example.com/v1/chat/completions?x=1",
                Some("http://proxy.internal:8080")
            )
            .expect("url should parse"),
            "https://api.example.com:443|proxy=http://proxy.internal:8080"
        );
    }

    #[test]
    fn target_queue_budget_defaults_to_short_budget() {
        assert_eq!(
            target_queue_budget(Duration::from_millis(250)),
            Duration::from_millis(DEFAULT_TARGET_QUEUE_BUDGET_MS)
        );
    }

    #[tokio::test]
    async fn acquire_times_out_when_target_gate_is_saturated() {
        let admission = UpstreamTargetAdmission::new(Some(1), Duration::from_millis(1));
        let plan = test_plan("http://127.0.0.1:18181/v1/chat/completions");
        let _first = admission
            .acquire(&plan, "trace-upstream-target")
            .await
            .expect("first acquire should succeed")
            .expect("gate enabled");

        let err = admission
            .acquire(&plan, "trace-upstream-target")
            .await
            .expect_err("second acquire should time out");

        assert!(matches!(
            err,
            GatewayError::AdmissionTimeout {
                gate: "gateway_upstream_target",
                ..
            }
        ));
        let snapshot = admission
            .snapshot_for_plan(&plan)
            .expect("target snapshot should exist");
        assert_eq!(snapshot.in_flight, 1);
        assert_eq!(snapshot.selected_total, 2);
        assert_eq!(snapshot.saturated_total, 1);
    }

    #[test]
    fn try_acquire_returns_none_when_target_is_saturated() {
        let admission = UpstreamTargetAdmission::new(Some(1), Duration::from_millis(1));
        let plan = test_plan("http://127.0.0.1:18181/v1/chat/completions");
        let _first = admission
            .try_acquire_for_plan(&plan)
            .expect("first try acquire should not error")
            .expect("first permit should be acquired");

        assert!(admission
            .try_acquire_for_plan(&plan)
            .expect("saturated try acquire should not error")
            .is_none());

        let snapshot = admission
            .snapshot_for_plan(&plan)
            .expect("target snapshot should exist");
        assert_eq!(snapshot.in_flight, 1);
        assert_eq!(snapshot.selected_total, 2);
        assert_eq!(snapshot.saturated_total, 1);
        let samples = admission.metric_samples();
        assert!(samples
            .iter()
            .any(|sample| sample.name == "upstream_target_selected_total"));
        assert!(samples
            .iter()
            .any(|sample| sample.name == "upstream_target_saturated_total"));
    }

    #[test]
    fn preselect_records_selection_pressure_before_acquire() {
        let admission = UpstreamTargetAdmission::new(Some(10), Duration::from_millis(1));
        let target = "http://127.0.0.1:18181|proxy=-";

        admission.record_preselect_for_target_key(target);
        admission.record_preselect_for_target_key(target);

        let snapshot = admission
            .snapshot_for_target_key(target)
            .expect("target snapshot should exist");
        assert_eq!(snapshot.in_flight, 0);
        assert_eq!(snapshot.raw_seen_total, 0);
        assert_eq!(snapshot.preselect_total, 2);
        assert_eq!(snapshot.selected_total, 0);
        assert_eq!(snapshot.selection_pressure_total, 2);
    }

    #[test]
    fn raw_seen_records_target_without_acquire() {
        let admission = UpstreamTargetAdmission::new(Some(10), Duration::from_millis(1));
        let target = "http://127.0.0.1:18182|proxy=-";

        admission.record_raw_seen_for_target_key(target);

        let snapshot = admission
            .snapshot_for_target_key(target)
            .expect("target snapshot should exist");
        assert_eq!(snapshot.in_flight, 0);
        assert_eq!(snapshot.raw_seen_total, 1);
        assert_eq!(snapshot.preselect_total, 0);
        assert_eq!(snapshot.selected_total, 0);
    }
}
