use super::classifier::{classify_local_failover, LocalFailoverClassification, LocalFailoverInput};
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
        | LocalFailoverClassification::StopErrorPattern => LocalFailoverDecision::StopLocalFailover,
        LocalFailoverClassification::RetrySuccessPattern
        | LocalFailoverClassification::RetryStatusCode
        | LocalFailoverClassification::RetryUpstreamFailure => {
            LocalFailoverDecision::RetryNextCandidate
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{analyze_local_failover, recover_local_failover_decision, LocalFailoverDecision};
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
}
