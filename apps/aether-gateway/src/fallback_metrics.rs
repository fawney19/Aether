use std::collections::BTreeMap;
use std::sync::Mutex;

use aether_runtime::{MetricKind, MetricLabel, MetricSample};

use crate::control::GatewayControlDecision;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum GatewayFallbackMetricKind {
    DecisionRemote,
    PlanFallback,
    ControlExecuteFallback,
    LocalExecutionRuntimeMiss,
    RemoteExecuteEmergency,
}

impl GatewayFallbackMetricKind {
    fn metric_name(self) -> &'static str {
        match self {
            Self::DecisionRemote => "decision_remote_total",
            Self::PlanFallback => "plan_fallback_total",
            Self::ControlExecuteFallback => "control_execute_fallback_total",
            Self::LocalExecutionRuntimeMiss => "local_execution_runtime_miss_total",
            Self::RemoteExecuteEmergency => "remote_execute_emergency_total",
        }
    }

    fn help(self) -> &'static str {
        match self {
            Self::DecisionRemote => {
                "Number of requests that fell back to Python decision endpoints."
            }
            Self::PlanFallback => "Number of requests that fell back to Python plan endpoints.",
            Self::ControlExecuteFallback => {
                "Number of requests that fell back to Python control execution."
            }
            Self::LocalExecutionRuntimeMiss => {
                "Number of requests that were terminated locally after execution runtime miss because no proxy fallback exists."
            }
            Self::RemoteExecuteEmergency => {
                "Number of requests that used remote emergency execution fallback."
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum GatewayFallbackReason {
    LocalDecisionMiss,
    RemoteDecisionMiss,
    SchedulerDecisionUnsupported,
    ExecutionRuntimeMiss,
    ProxyPassthrough,
    LocalExecutionPathRequired,
    ControlExecuteEmergency,
    ExecutionRuntimeMissing,
}

impl GatewayFallbackReason {
    pub(crate) fn as_label_value(self) -> &'static str {
        match self {
            Self::LocalDecisionMiss => "local_decision_miss",
            Self::RemoteDecisionMiss => "remote_decision_miss",
            Self::SchedulerDecisionUnsupported => "scheduler_decision_unsupported",
            Self::ExecutionRuntimeMiss => "execution_runtime_miss",
            Self::ProxyPassthrough => "proxy_passthrough",
            Self::LocalExecutionPathRequired => "local_execution_path_required",
            Self::ControlExecuteEmergency => "control_execute_emergency",
            Self::ExecutionRuntimeMissing => "execution_runtime_missing",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct GatewayFallbackMetricKey {
    kind: GatewayFallbackMetricKind,
    route_class: String,
    route_family: String,
    route_kind: String,
    plan_kind: String,
    execution_path: String,
    reason: GatewayFallbackReason,
}

#[derive(Debug, Default)]
pub(crate) struct GatewayFallbackMetrics {
    counters: Mutex<BTreeMap<GatewayFallbackMetricKey, u64>>,
}

impl GatewayFallbackMetrics {
    pub(crate) fn record(
        &self,
        kind: GatewayFallbackMetricKind,
        decision: Option<&GatewayControlDecision>,
        plan_kind: Option<&str>,
        execution_path: Option<&str>,
        reason: GatewayFallbackReason,
    ) {
        let key = GatewayFallbackMetricKey {
            kind,
            route_class: decision
                .and_then(|decision| decision.route_class.as_deref())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("unknown")
                .to_string(),
            route_family: decision
                .and_then(|decision| decision.route_family.as_deref())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("unknown")
                .to_string(),
            route_kind: decision
                .and_then(|decision| decision.route_kind.as_deref())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("unknown")
                .to_string(),
            plan_kind: plan_kind
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("none")
                .to_string(),
            execution_path: execution_path
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("none")
                .to_string(),
            reason,
        };

        let mut counters = self
            .counters
            .lock()
            .expect("fallback metrics lock poisoned");
        *counters.entry(key).or_default() += 1;
    }

    pub(crate) fn metric_samples(&self) -> Vec<MetricSample> {
        self.counters
            .lock()
            .expect("fallback metrics lock poisoned")
            .iter()
            .map(|(key, value)| {
                MetricSample::new(
                    key.kind.metric_name(),
                    key.kind.help(),
                    MetricKind::Counter,
                    *value,
                )
                .with_labels(vec![
                    MetricLabel::new("route_class", key.route_class.clone()),
                    MetricLabel::new("route_family", key.route_family.clone()),
                    MetricLabel::new("route_kind", key.route_kind.clone()),
                    MetricLabel::new("plan_kind", key.plan_kind.clone()),
                    MetricLabel::new("execution_path", key.execution_path.clone()),
                    MetricLabel::new("reason", key.reason.as_label_value()),
                ])
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{GatewayFallbackMetricKind, GatewayFallbackMetrics, GatewayFallbackReason};
    use crate::control::GatewayControlDecision;

    fn sample_decision() -> GatewayControlDecision {
        GatewayControlDecision {
            public_path: "/v1/chat/completions".to_string(),
            public_query_string: None,
            route_class: Some("ai_public".to_string()),
            route_family: Some("openai".to_string()),
            route_kind: Some("chat".to_string()),
            request_auth_channel: None,
            auth_endpoint_signature: None,
            execution_runtime_candidate: true,
            auth_context: None,
            admin_principal: None,
            local_auth_rejection: None,
        }
    }

    #[test]
    fn records_and_renders_fallback_metric_samples() {
        let metrics = GatewayFallbackMetrics::default();
        let decision = sample_decision();

        metrics.record(
            GatewayFallbackMetricKind::DecisionRemote,
            Some(&decision),
            Some("openai_chat_sync"),
            None,
            GatewayFallbackReason::LocalDecisionMiss,
        );
        metrics.record(
            GatewayFallbackMetricKind::DecisionRemote,
            Some(&decision),
            Some("openai_chat_sync"),
            None,
            GatewayFallbackReason::LocalDecisionMiss,
        );

        let samples = metrics.metric_samples();
        assert_eq!(samples.len(), 1);
        let sample = &samples[0];
        assert_eq!(sample.name, "decision_remote_total");
        assert_eq!(sample.value, 2);
        assert!(sample
            .labels
            .iter()
            .any(|label| { label.key == "route_class" && label.value == "ai_public" }));
        assert!(sample
            .labels
            .iter()
            .any(|label| { label.key == "route_family" && label.value == "openai" }));
        assert!(sample
            .labels
            .iter()
            .any(|label| { label.key == "route_kind" && label.value == "chat" }));
        assert!(sample
            .labels
            .iter()
            .any(|label| { label.key == "plan_kind" && label.value == "openai_chat_sync" }));
        assert!(sample
            .labels
            .iter()
            .any(|label| { label.key == "execution_path" && label.value == "none" }));
        assert!(sample
            .labels
            .iter()
            .any(|label| { label.key == "reason" && label.value == "local_decision_miss" }));
    }
}
