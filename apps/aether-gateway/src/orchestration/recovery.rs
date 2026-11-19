use super::classifier::{
    classify_failure_disposition, classify_local_failover, classify_local_transport_error,
    FailureRetryAction, LocalFailoverClassification, LocalFailoverInput,
    LocalTransportFailoverClassification,
};
use super::LocalFailoverPolicy;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalFailoverDecision {
    UseDefault,
    RetryNextCandidate,
    StopLocalFailover,
}

impl LocalFailoverDecision {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::UseDefault => "use_default",
            Self::RetryNextCandidate => "retry_next_candidate",
            Self::StopLocalFailover => "stop_local_failover",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LocalFailoverAnalysis {
    pub(crate) classification: LocalFailoverClassification,
    pub(crate) decision: LocalFailoverDecision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LocalTransportFailoverAnalysis {
    pub(crate) classification: LocalTransportFailoverClassification,
    pub(crate) decision: LocalFailoverDecision,
}

impl LocalFailoverAnalysis {
    pub(crate) const fn use_default() -> Self {
        Self {
            classification: LocalFailoverClassification::UseDefault,
            decision: LocalFailoverDecision::UseDefault,
        }
    }
}

pub(crate) fn analyze_local_failover(
    policy: &LocalFailoverPolicy,
    input: LocalFailoverInput<'_>,
) -> LocalFailoverAnalysis {
    let classification = classify_local_failover(policy, input);
    LocalFailoverAnalysis {
        classification,
        decision: decision_from_classification(classification),
    }
}

pub(crate) fn analyze_local_transport_error(
    policy: &LocalFailoverPolicy,
) -> LocalTransportFailoverAnalysis {
    let classification = classify_local_transport_error(policy);
    let decision = match classification {
        LocalTransportFailoverClassification::StopTransportError => {
            LocalFailoverDecision::StopLocalFailover
        }
        LocalTransportFailoverClassification::RetryTransportError => {
            LocalFailoverDecision::RetryNextCandidate
        }
    };
    LocalTransportFailoverAnalysis {
        classification,
        decision,
    }
}

pub(crate) fn apply_provider_failure_disposition(
    provider_api_format: &str,
    status_code: u16,
    analysis: LocalFailoverAnalysis,
) -> LocalFailoverAnalysis {
    if status_code < 400
        && matches!(
            analysis.classification,
            LocalFailoverClassification::UseDefault
        )
    {
        return analysis;
    }

    let disposition =
        classify_failure_disposition(provider_api_format, analysis.classification, status_code);
    let decision = match disposition.retry_action {
        FailureRetryAction::Stop | FailureRetryAction::SameCredential => {
            LocalFailoverDecision::StopLocalFailover
        }
        FailureRetryAction::NextCandidate
        | FailureRetryAction::NextCredential
        | FailureRetryAction::NextEndpoint => LocalFailoverDecision::RetryNextCandidate,
    };

    LocalFailoverAnalysis {
        classification: analysis.classification,
        decision,
    }
}

pub(crate) fn recover_local_failover_decision(
    policy: &LocalFailoverPolicy,
    input: LocalFailoverInput<'_>,
) -> LocalFailoverDecision {
    analyze_local_failover(policy, input).decision
}

const fn decision_from_classification(
    classification: LocalFailoverClassification,
) -> LocalFailoverDecision {
    match classification {
        LocalFailoverClassification::UseDefault => LocalFailoverDecision::UseDefault,
        LocalFailoverClassification::StopStatusCode
        | LocalFailoverClassification::StopErrorPattern
        | LocalFailoverClassification::StopExecutionError
        | LocalFailoverClassification::StopCyberPolicy
        | LocalFailoverClassification::StopContextOverflow => {
            LocalFailoverDecision::StopLocalFailover
        }
        LocalFailoverClassification::RetrySuccessPattern
        | LocalFailoverClassification::RetryStatusCode
        | LocalFailoverClassification::RetryUpstreamFailure => {
            LocalFailoverDecision::RetryNextCandidate
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        analyze_local_failover, analyze_local_transport_error, apply_provider_failure_disposition,
        recover_local_failover_decision, LocalFailoverAnalysis, LocalFailoverDecision,
    };
    use crate::orchestration::{
        LocalFailoverClassification, LocalFailoverInput, LocalFailoverPolicy,
    };

    #[test]
    fn recovery_maps_retryable_status_to_retry_next_candidate() {
        let policy = LocalFailoverPolicy {
            continue_status_codes: [429].into_iter().collect(),
            ..LocalFailoverPolicy::default()
        };

        assert_eq!(
            recover_local_failover_decision(&policy, LocalFailoverInput::new(429, None)),
            LocalFailoverDecision::RetryNextCandidate
        );
    }

    #[test]
    fn recovery_maps_neutral_status_to_use_default() {
        assert_eq!(
            recover_local_failover_decision(
                &LocalFailoverPolicy::default(),
                LocalFailoverInput::new(200, None)
            ),
            LocalFailoverDecision::UseDefault
        );
    }

    #[test]
    fn transport_error_recovery_defaults_to_retry_and_can_stop() {
        assert_eq!(
            analyze_local_transport_error(&LocalFailoverPolicy::default()).decision,
            LocalFailoverDecision::RetryNextCandidate
        );

        let stop_policy = LocalFailoverPolicy {
            stop_on_transport_errors: true,
            ..LocalFailoverPolicy::default()
        };
        assert_eq!(
            analyze_local_transport_error(&stop_policy).decision,
            LocalFailoverDecision::StopLocalFailover
        );
    }

    #[test]
    fn recovery_retries_default_client_error_without_custom_rule() {
        assert_eq!(
            recover_local_failover_decision(
                &LocalFailoverPolicy::default(),
                LocalFailoverInput::new(
                    400,
                    Some("{\"error\":{\"type\":\"invalid_request_error\",\"message\":\"prompt is too long\"}}")
                )
            ),
            LocalFailoverDecision::RetryNextCandidate
        );
    }

    #[test]
    fn recovery_retries_any_error_status_without_custom_rule() {
        assert_eq!(
            recover_local_failover_decision(
                &LocalFailoverPolicy::default(),
                LocalFailoverInput::new(
                    400,
                    Some("{\"error\":{\"message\":\"invalid `signature` in `thinking` block\"}}")
                )
            ),
            LocalFailoverDecision::RetryNextCandidate
        );
    }

    #[test]
    fn analysis_keeps_classification_and_decision_together() {
        let analysis = analyze_local_failover(
            &LocalFailoverPolicy::default(),
            LocalFailoverInput::new(
                400,
                Some("{\"error\":{\"message\":\"Unsupported parameter: stream_options is not supported with this model\"}}"),
            ),
        );

        assert_eq!(analysis.decision, LocalFailoverDecision::RetryNextCandidate);
        assert_eq!(
            analysis.classification,
            LocalFailoverClassification::RetryUpstreamFailure
        );
    }

    #[test]
    fn recovery_stops_cyber_policy_failover() {
        let policy = LocalFailoverPolicy {
            stop_cyber_policy_errors: true,
            ..LocalFailoverPolicy::default()
        };
        let analysis = analyze_local_failover(
            &policy,
            LocalFailoverInput::new(400, Some(r#"{"error":{"code":"cyber_policy"}}"#)),
        );

        assert_eq!(analysis.decision, LocalFailoverDecision::StopLocalFailover);
        assert_eq!(
            analysis.classification,
            LocalFailoverClassification::StopCyberPolicy
        );
    }

    #[test]
    fn anthropic_failure_disposition_controls_candidate_retry() {
        let policy = LocalFailoverPolicy::default();

        for status_code in [400, 413] {
            let analysis = analyze_local_failover(
                &policy,
                LocalFailoverInput::new(status_code, Some(r#"{"error":{"message":"failed"}}"#)),
            );
            assert_eq!(
                apply_provider_failure_disposition("claude:messages", status_code, analysis,)
                    .decision,
                LocalFailoverDecision::StopLocalFailover,
                "Anthropic status {status_code} must not blindly rotate credentials"
            );
        }

        for status_code in [401, 403, 404, 429, 529] {
            let analysis = analyze_local_failover(
                &policy,
                LocalFailoverInput::new(status_code, Some(r#"{"error":{"message":"failed"}}"#)),
            );
            assert_eq!(
                apply_provider_failure_disposition("claude:messages", status_code, analysis,)
                    .decision,
                LocalFailoverDecision::RetryNextCandidate,
                "Anthropic status {status_code} should continue candidate failover"
            );
        }
    }

    #[test]
    fn provider_failure_disposition_preserves_non_failure_default() {
        let analysis = LocalFailoverAnalysis::use_default();
        assert_eq!(
            apply_provider_failure_disposition("claude:messages", 200, analysis).decision,
            LocalFailoverDecision::UseDefault
        );
    }
}
