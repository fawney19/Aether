use regex::Regex;
use serde_json::Value;

use super::{LocalFailoverPolicy, LocalFailoverRegexRule};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ParsedLocalErrorResponse {
    message: Option<String>,
    reason: Option<String>,
    raw: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LocalFailoverInput<'a> {
    pub(crate) status_code: u16,
    pub(crate) response_text: Option<&'a str>,
}

impl<'a> LocalFailoverInput<'a> {
    pub(crate) fn new(status_code: u16, response_text: Option<&'a str>) -> Self {
        Self {
            status_code,
            response_text: response_text
                .map(str::trim)
                .filter(|value| !value.is_empty()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalFailoverClassification {
    UseDefault,
    StopStatusCode,
    StopErrorPattern,
    StopExecutionError,
    StopCyberPolicy,
    RetrySuccessPattern,
    RetryStatusCode,
    RetryUpstreamFailure,
}

impl LocalFailoverClassification {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::UseDefault => "use_default",
            Self::StopStatusCode => "stop_status_code",
            Self::StopErrorPattern => "stop_error_pattern",
            Self::StopExecutionError => "stop_execution_error",
            Self::StopCyberPolicy => "stop_cyber_policy",
            Self::RetrySuccessPattern => "retry_success_pattern",
            Self::RetryStatusCode => "retry_status_code",
            Self::RetryUpstreamFailure => "retry_upstream_failure",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalTransportFailoverClassification {
    StopTransportError,
    RetryTransportError,
}

impl LocalTransportFailoverClassification {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::StopTransportError => "stop_transport_error",
            Self::RetryTransportError => "retry_transport_error",
        }
    }
}

pub(crate) const fn classify_local_transport_error(
    policy: &LocalFailoverPolicy,
) -> LocalTransportFailoverClassification {
    if policy.stop_on_transport_errors {
        LocalTransportFailoverClassification::StopTransportError
    } else {
        LocalTransportFailoverClassification::RetryTransportError
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureRetryAction {
    Stop,
    SameCredential,
    NextCandidate,
    NextCredential,
    NextEndpoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureScope {
    None,
    Credential,
    CredentialModel,
    Endpoint,
    Provider,
}

impl FailureScope {
    pub(crate) const fn affects_credential(self) -> bool {
        matches!(self, Self::Credential | Self::CredentialModel)
    }

    pub(crate) const fn allows_key_wide_effects(self) -> bool {
        matches!(self, Self::None | Self::Credential)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureTokenAction {
    None,
    ForceRefresh,
    #[allow(dead_code)]
    Quarantine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FailureDisposition {
    pub(crate) retry_action: FailureRetryAction,
    pub(crate) failure_scope: FailureScope,
    pub(crate) token_action: FailureTokenAction,
    pub(crate) preserve_upstream_error: bool,
}

impl FailureDisposition {
    const fn new(
        retry_action: FailureRetryAction,
        failure_scope: FailureScope,
        token_action: FailureTokenAction,
        preserve_upstream_error: bool,
    ) -> Self {
        Self {
            retry_action,
            failure_scope,
            token_action,
            preserve_upstream_error,
        }
    }
}

pub(crate) const fn failure_disposition_from_local_classification(
    classification: LocalFailoverClassification,
    status_code: u16,
) -> FailureDisposition {
    match classification {
        LocalFailoverClassification::StopStatusCode
        | LocalFailoverClassification::StopErrorPattern
        | LocalFailoverClassification::StopExecutionError
        | LocalFailoverClassification::StopCyberPolicy => FailureDisposition::new(
            FailureRetryAction::Stop,
            FailureScope::None,
            FailureTokenAction::None,
            true,
        ),
        LocalFailoverClassification::UseDefault => FailureDisposition::new(
            FailureRetryAction::Stop,
            FailureScope::None,
            FailureTokenAction::None,
            status_code >= 400,
        ),
        LocalFailoverClassification::RetrySuccessPattern => FailureDisposition::new(
            FailureRetryAction::NextCandidate,
            FailureScope::None,
            FailureTokenAction::None,
            false,
        ),
        LocalFailoverClassification::RetryStatusCode
        | LocalFailoverClassification::RetryUpstreamFailure => FailureDisposition::new(
            FailureRetryAction::NextCandidate,
            FailureScope::None,
            FailureTokenAction::None,
            false,
        ),
    }
}

pub(crate) const fn classify_anthropic_failure_disposition(
    classification: LocalFailoverClassification,
    status_code: u16,
) -> FailureDisposition {
    if matches!(
        classification,
        LocalFailoverClassification::StopStatusCode
            | LocalFailoverClassification::StopErrorPattern
            | LocalFailoverClassification::StopExecutionError
            | LocalFailoverClassification::StopCyberPolicy
    ) {
        let generic = failure_disposition_from_local_classification(classification, status_code);
        return match status_code {
            401 => FailureDisposition::new(
                generic.retry_action,
                FailureScope::Credential,
                FailureTokenAction::ForceRefresh,
                generic.preserve_upstream_error,
            ),
            403 => FailureDisposition::new(
                generic.retry_action,
                FailureScope::Credential,
                FailureTokenAction::None,
                generic.preserve_upstream_error,
            ),
            404 => FailureDisposition::new(
                generic.retry_action,
                FailureScope::Endpoint,
                FailureTokenAction::None,
                generic.preserve_upstream_error,
            ),
            429 => FailureDisposition::new(
                generic.retry_action,
                FailureScope::CredentialModel,
                FailureTokenAction::None,
                generic.preserve_upstream_error,
            ),
            529 => FailureDisposition::new(
                generic.retry_action,
                FailureScope::Provider,
                FailureTokenAction::None,
                generic.preserve_upstream_error,
            ),
            500..=599 => FailureDisposition::new(
                generic.retry_action,
                FailureScope::Endpoint,
                FailureTokenAction::None,
                generic.preserve_upstream_error,
            ),
            _ => generic,
        };
    }

    match status_code {
        400 => FailureDisposition::new(
            FailureRetryAction::Stop,
            FailureScope::None,
            FailureTokenAction::None,
            true,
        ),
        401 => FailureDisposition::new(
            FailureRetryAction::NextCredential,
            FailureScope::Credential,
            FailureTokenAction::ForceRefresh,
            true,
        ),
        403 => FailureDisposition::new(
            FailureRetryAction::NextCredential,
            FailureScope::Credential,
            FailureTokenAction::None,
            true,
        ),
        404 => FailureDisposition::new(
            FailureRetryAction::NextEndpoint,
            FailureScope::Endpoint,
            FailureTokenAction::None,
            true,
        ),
        413 => FailureDisposition::new(
            FailureRetryAction::Stop,
            FailureScope::None,
            FailureTokenAction::None,
            true,
        ),
        429 => FailureDisposition::new(
            FailureRetryAction::NextCredential,
            FailureScope::CredentialModel,
            FailureTokenAction::None,
            true,
        ),
        529 => FailureDisposition::new(
            FailureRetryAction::NextEndpoint,
            FailureScope::Provider,
            FailureTokenAction::None,
            true,
        ),
        500..=599 => FailureDisposition::new(
            FailureRetryAction::NextEndpoint,
            FailureScope::Endpoint,
            FailureTokenAction::None,
            true,
        ),
        _ => failure_disposition_from_local_classification(classification, status_code),
    }
}

pub(crate) fn classify_failure_disposition(
    provider_api_format: &str,
    classification: LocalFailoverClassification,
    status_code: u16,
) -> FailureDisposition {
    if provider_api_format
        .trim()
        .eq_ignore_ascii_case("claude:messages")
    {
        classify_anthropic_failure_disposition(classification, status_code)
    } else {
        failure_disposition_from_local_classification(classification, status_code)
    }
}

pub(crate) fn classify_local_failover(
    policy: &LocalFailoverPolicy,
    input: LocalFailoverInput<'_>,
) -> LocalFailoverClassification {
    if input.status_code >= 400
        && policy.routing_rules.error_stop_patterns.iter().any(|rule| {
            failover_pattern_matches(
                &rule.pattern,
                &rule.status_codes,
                input.response_text,
                input.status_code,
            )
        })
    {
        return LocalFailoverClassification::StopErrorPattern;
    }
    if input.status_code == 200
        && policy
            .routing_rules
            .success_failover_patterns
            .iter()
            .any(|rule| {
                failover_pattern_matches(
                    &rule.pattern,
                    &rule.status_codes,
                    input.response_text,
                    input.status_code,
                )
            })
    {
        return LocalFailoverClassification::RetrySuccessPattern;
    }
    if policy.stop_status_codes.contains(&input.status_code) {
        return LocalFailoverClassification::StopStatusCode;
    }

    if policy.stop_cyber_policy_errors
        && input.status_code >= 400
        && local_error_response_has_cyber_policy_code(input.response_text)
    {
        return LocalFailoverClassification::StopCyberPolicy;
    }

    if input.status_code >= 400
        && policy.error_stop_patterns.iter().any(|rule| {
            local_failover_regex_rule_matches(rule, input.response_text, input.status_code)
        })
    {
        return LocalFailoverClassification::StopErrorPattern;
    }

    if input.status_code == 200
        && input.response_text.is_some_and(|text| {
            policy
                .success_failover_patterns
                .iter()
                .any(|rule| local_failover_regex_rule_matches(rule, Some(text), input.status_code))
        })
    {
        return LocalFailoverClassification::RetrySuccessPattern;
    }

    if policy.continue_status_codes.contains(&input.status_code) {
        return LocalFailoverClassification::RetryStatusCode;
    }

    if should_failover_local_upstream_status(
        input.status_code,
        policy.retry_client_errors_by_default,
    ) {
        return LocalFailoverClassification::RetryUpstreamFailure;
    }

    LocalFailoverClassification::UseDefault
}

pub(crate) fn local_failover_error_message(response_text: Option<&str>) -> Option<String> {
    let parsed = parse_local_error_response(response_text);
    parsed
        .message
        .or(parsed.reason)
        .or(parsed.raw)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn should_failover_local_upstream_status(
    status_code: u16,
    retry_client_errors_by_default: bool,
) -> bool {
    status_code >= 500 || status_code >= 400 && retry_client_errors_by_default
}

fn local_error_response_has_cyber_policy_code(response_text: Option<&str>) -> bool {
    let Some(response_text) = response_text else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<Value>(response_text) else {
        return false;
    };
    json_value_has_cyber_policy_code(&value, 0)
}

fn json_value_has_cyber_policy_code(value: &Value, depth: usize) -> bool {
    if depth > 16 {
        return false;
    }
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            (key.eq_ignore_ascii_case("code") && value.as_str().is_some_and(is_cyber_policy_code))
                || json_value_has_cyber_policy_code(value, depth + 1)
        }),
        Value::Array(values) => values
            .iter()
            .any(|value| json_value_has_cyber_policy_code(value, depth + 1)),
        Value::String(text) => {
            let text = text.trim_start();
            if !text.starts_with('{') && !text.starts_with('[') {
                return false;
            }
            serde_json::from_str::<Value>(text)
                .ok()
                .is_some_and(|value| json_value_has_cyber_policy_code(&value, depth + 1))
        }
        _ => false,
    }
}

fn is_cyber_policy_code(code: &str) -> bool {
    let code = code.trim();
    code.eq_ignore_ascii_case("cyber_policy") || code.eq_ignore_ascii_case("cyber_policy_violation")
}

fn parse_local_error_response(response_text: Option<&str>) -> ParsedLocalErrorResponse {
    let raw = response_text
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let Some(raw_text) = raw.clone() else {
        return ParsedLocalErrorResponse::default();
    };

    let mut parsed = ParsedLocalErrorResponse {
        raw: Some(raw_text.clone()),
        ..ParsedLocalErrorResponse::default()
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw_text) else {
        parsed.message = Some(raw_text);
        return parsed;
    };

    let body_object = value.as_object();
    let error_object = body_object
        .and_then(|object| object.get("error"))
        .and_then(Value::as_object);

    parsed.message = first_non_empty_json_text(error_object, &["message", "detail", "reason"])
        .or_else(|| first_non_empty_json_text(body_object, &["errorMessage"]))
        .or_else(|| {
            body_object
                .and_then(|object| object.get("error"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
        .or_else(|| first_non_empty_json_text(body_object, &["message", "detail", "reason"]));
    parsed.reason = first_non_empty_json_text(error_object, &["reason", "code", "status"])
        .or_else(|| first_non_empty_json_text(body_object, &["reason", "code", "status"]));

    let Some(message) = parsed.message.clone() else {
        return parsed;
    };
    if !message.starts_with('{') {
        return parsed;
    }

    let Ok(nested) = serde_json::from_str::<Value>(&message) else {
        return parsed;
    };
    let nested_object = nested.as_object();
    let nested_error_object = nested_object
        .and_then(|object| object.get("error"))
        .and_then(Value::as_object);
    parsed.message =
        first_non_empty_json_text(nested_error_object, &["message", "detail", "reason"])
            .or_else(|| first_non_empty_json_text(nested_object, &["message", "detail", "reason"]))
            .or(parsed.message);
    parsed.reason = parsed
        .reason
        .or_else(|| first_non_empty_json_text(nested_error_object, &["reason", "code", "status"]))
        .or_else(|| first_non_empty_json_text(nested_object, &["reason", "code", "status"]));

    parsed
}

fn first_non_empty_json_text(
    object: Option<&serde_json::Map<String, Value>>,
    keys: &[&str],
) -> Option<String> {
    let object = object?;
    for key in keys {
        let Some(value) = object.get(*key) else {
            continue;
        };
        match value {
            Value::String(text) if !text.trim().is_empty() => return Some(text.trim().to_string()),
            Value::Number(number) => return Some(number.to_string()),
            _ => {}
        }
    }
    None
}

fn local_failover_regex_rule_matches(
    rule: &LocalFailoverRegexRule,
    response_text: Option<&str>,
    status_code: u16,
) -> bool {
    failover_pattern_matches(
        &rule.pattern,
        &rule.status_codes,
        response_text,
        status_code,
    )
}

fn failover_pattern_matches(
    pattern: &str,
    status_codes: &std::collections::BTreeSet<u16>,
    response_text: Option<&str>,
    status_code: u16,
) -> bool {
    if !status_codes.is_empty() && !status_codes.contains(&status_code) {
        return false;
    }

    let pattern = pattern.trim();
    if pattern.is_empty() {
        return !status_codes.is_empty();
    }

    let Some(response_text) = response_text else {
        return false;
    };

    Regex::new(pattern)
        .ok()
        .is_some_and(|regex| regex.is_match(response_text))
}

#[cfg(test)]
mod tests {
    #[test]
    fn routing_rules_precede_provider_rules_and_keep_provider_fallback() {
        let policy = super::LocalFailoverPolicy {
            routing_rules: aether_routing_core::RoutingFailoverRules {
                success_failover_patterns: vec![aether_routing_core::RoutingFailoverRule {
                    pattern: "(?i)capacity.*exhausted".to_string(),
                    ..Default::default()
                }],
                error_stop_patterns: vec![aether_routing_core::RoutingFailoverRule {
                    pattern: "invalid.*parameter".to_string(),
                    status_codes: [400].into_iter().collect(),
                }],
            },
            stop_status_codes: [200, 403].into_iter().collect(),
            continue_status_codes: [400].into_iter().collect(),
            ..Default::default()
        };
        for (status, body, expected) in [
            (
                200,
                "CAPACITY exhausted",
                super::LocalFailoverClassification::RetrySuccessPattern,
            ),
            (
                400,
                "invalid request parameter",
                super::LocalFailoverClassification::StopErrorPattern,
            ),
            (
                400,
                "capacity exhausted",
                super::LocalFailoverClassification::RetryStatusCode,
            ),
            (
                403,
                "permission denied",
                super::LocalFailoverClassification::StopStatusCode,
            ),
            (
                429,
                "rate limited",
                super::LocalFailoverClassification::RetryUpstreamFailure,
            ),
        ] {
            assert_eq!(
                super::classify_local_failover(
                    &policy,
                    super::LocalFailoverInput::new(status, Some(body))
                ),
                expected
            );
        }
    }

    #[test]
    fn provider_transport_stop_rule_is_respected() {
        let policy = super::LocalFailoverPolicy {
            stop_on_transport_errors: true,
            ..Default::default()
        };
        assert_eq!(
            super::classify_local_transport_error(&policy),
            super::LocalTransportFailoverClassification::StopTransportError
        );
    }
    use std::collections::BTreeSet;

    use super::{
        classify_anthropic_failure_disposition, classify_local_failover,
        classify_local_transport_error, failure_disposition_from_local_classification,
        FailureDisposition, FailureRetryAction, FailureScope, FailureTokenAction,
        LocalFailoverClassification, LocalFailoverInput, LocalTransportFailoverClassification,
    };
    use crate::orchestration::{LocalFailoverPolicy, LocalFailoverRegexRule};

    #[test]
    fn classifier_honors_explicit_stop_before_default_retryable_status() {
        let policy = LocalFailoverPolicy {
            stop_status_codes: [503].into_iter().collect(),
            ..LocalFailoverPolicy::default()
        };

        assert_eq!(
            classify_local_failover(&policy, LocalFailoverInput::new(503, None)),
            LocalFailoverClassification::StopStatusCode
        );
    }

    #[test]
    fn classifier_retries_transport_errors_by_default_and_honors_explicit_stop() {
        assert_eq!(
            classify_local_transport_error(&LocalFailoverPolicy::default()),
            LocalTransportFailoverClassification::RetryTransportError
        );

        let stop_policy = LocalFailoverPolicy {
            stop_on_transport_errors: true,
            ..LocalFailoverPolicy::default()
        };
        assert_eq!(
            classify_local_transport_error(&stop_policy),
            LocalTransportFailoverClassification::StopTransportError
        );
        assert_eq!(
            classify_local_transport_error(&stop_policy).as_str(),
            "stop_transport_error"
        );
    }

    #[test]
    fn classifier_detects_success_failover_pattern() {
        let policy = LocalFailoverPolicy {
            success_failover_patterns: vec![LocalFailoverRegexRule {
                pattern: "relay:.*格式错误".to_string(),
                status_codes: BTreeSet::new(),
            }],
            ..LocalFailoverPolicy::default()
        };

        assert_eq!(
            classify_local_failover(
                &policy,
                LocalFailoverInput::new(200, Some("{\"error\":\"relay: 返回格式错误\"}"))
            ),
            LocalFailoverClassification::RetrySuccessPattern
        );
    }

    #[test]
    fn classifier_detects_error_stop_pattern() {
        let policy = LocalFailoverPolicy {
            error_stop_patterns: vec![LocalFailoverRegexRule {
                pattern: "content_policy_violation".to_string(),
                status_codes: [400, 403].into_iter().collect(),
            }],
            ..LocalFailoverPolicy::default()
        };

        assert_eq!(
            classify_local_failover(
                &policy,
                LocalFailoverInput::new(400, Some("{\"error\":\"content_policy_violation\"}"))
            ),
            LocalFailoverClassification::StopErrorPattern
        );
    }

    #[test]
    fn classifier_detects_error_stop_pattern_without_status_codes_on_any_error_status() {
        let policy = LocalFailoverPolicy {
            error_stop_patterns: vec![LocalFailoverRegexRule {
                pattern: "content_policy_violation".to_string(),
                status_codes: BTreeSet::new(),
            }],
            ..LocalFailoverPolicy::default()
        };

        for status_code in [400, 429, 503] {
            assert_eq!(
                classify_local_failover(
                    &policy,
                    LocalFailoverInput::new(
                        status_code,
                        Some("{\"error\":\"content_policy_violation\"}")
                    )
                ),
                LocalFailoverClassification::StopErrorPattern
            );
        }
    }

    #[test]
    fn classifier_detects_status_only_error_stop_rule_without_response_text() {
        let policy = LocalFailoverPolicy {
            error_stop_patterns: vec![LocalFailoverRegexRule {
                pattern: String::new(),
                status_codes: [429].into_iter().collect(),
            }],
            ..LocalFailoverPolicy::default()
        };

        assert_eq!(
            classify_local_failover(&policy, LocalFailoverInput::new(429, None)),
            LocalFailoverClassification::StopErrorPattern
        );
        assert_eq!(
            classify_local_failover(&policy, LocalFailoverInput::new(503, None)),
            LocalFailoverClassification::RetryUpstreamFailure
        );
    }

    #[test]
    fn classifier_stops_cyber_policy_when_policy_enabled() {
        let policy = LocalFailoverPolicy {
            stop_cyber_policy_errors: true,
            ..LocalFailoverPolicy::default()
        };

        assert_eq!(
            classify_local_failover(
                &policy,
                LocalFailoverInput::new(
                    400,
                    Some(
                        r#"{"type":"error","error":{"type":"invalid_request","code":"cyber_policy","message":"flagged"}}"#,
                    )
                )
            ),
            LocalFailoverClassification::StopCyberPolicy
        );
        assert_eq!(
            classify_local_failover(
                &policy,
                LocalFailoverInput::new(
                    400,
                    Some(r#"{"error":{"code":"cyber_policy_violation"}}"#)
                )
            ),
            LocalFailoverClassification::StopCyberPolicy
        );
        assert_eq!(
            classify_local_failover(
                &policy,
                LocalFailoverInput::new(
                    400,
                    Some(r#"{"outer":{"error":{"code":"cyber_policy"}}}"#)
                )
            ),
            LocalFailoverClassification::StopCyberPolicy
        );
        assert_eq!(
            classify_local_failover(
                &policy,
                LocalFailoverInput::new(400, Some(r#"{"error":{"code":"other"}}"#))
            ),
            LocalFailoverClassification::RetryUpstreamFailure
        );
    }

    #[test]
    fn classifier_retries_cyber_policy_when_policy_disabled() {
        let policy = LocalFailoverPolicy {
            stop_cyber_policy_errors: false,
            ..LocalFailoverPolicy::default()
        };
        assert_eq!(
            classify_local_failover(
                &policy,
                LocalFailoverInput::new(
                    400,
                    Some(r#"{"error":{"code":"cyber_policy","message":"flagged"}}"#)
                )
            ),
            LocalFailoverClassification::RetryUpstreamFailure
        );
    }

    #[test]
    fn classifier_detects_success_continue_status_code() {
        let policy = LocalFailoverPolicy {
            continue_status_codes: [200].into_iter().collect(),
            ..LocalFailoverPolicy::default()
        };

        assert_eq!(
            classify_local_failover(&policy, LocalFailoverInput::new(200, None)),
            LocalFailoverClassification::RetryStatusCode
        );
    }

    #[test]
    fn classifier_retries_all_error_statuses_without_custom_rule() {
        for (status_code, response_text) in [
            (
                400,
                "{\"error\":{\"type\":\"invalid_request_error\",\"message\":\"prompt is too long\"}}",
            ),
            (
                400,
                "{\"error\":{\"message\":\"Unsupported parameter: max_tokens is not supported with this model\"}}",
            ),
            (
                400,
                "{\"error\":{\"message\":\"Unknown parameter: 'tools[0].n'.\"}}",
            ),
            (
                400,
                "{\"error\":{\"message\":\"invalid model for this endpoint\"}}",
            ),
            (
                400,
                "{\"error\":{\"message\":\"invalid `signature` in `thinking` block: signature is for a different request\"}}",
            ),
            (
                400,
                "{\"error\":{\"message\":\"resource_exhausted: quota reached\"}}",
            ),
            (
                401,
                "{\"error\":{\"type\":\"invalid_request_error\",\"message\":\"Your authentication token has been invalidated. Please try signing in again.\"}}",
            ),
            (
                402,
                "{\"error\":{\"type\":\"invalid_request_error\",\"message\":\"payment required: credit balance exhausted\"}}",
            ),
            (
                403,
                "{\"error\":{\"type\":\"invalid_request_error\",\"message\":\"verify your account before continuing\"}}",
            ),
            (429, "{\"error\":{\"message\":\"rate limited\"}}"),
            (500, "{\"error\":{\"message\":\"upstream failed\"}}"),
        ] {
            assert_eq!(
                classify_local_failover(
                    &LocalFailoverPolicy::default(),
                    LocalFailoverInput::new(status_code, Some(response_text))
                ),
                LocalFailoverClassification::RetryUpstreamFailure
            );
        }
    }

    #[test]
    fn classifier_passes_through_client_errors_when_protocol_default_disables_failover() {
        let policy = LocalFailoverPolicy {
            retry_client_errors_by_default: false,
            ..LocalFailoverPolicy::default()
        };

        for status_code in [400, 401, 429, 499] {
            assert_eq!(
                classify_local_failover(&policy, LocalFailoverInput::new(status_code, None)),
                LocalFailoverClassification::UseDefault
            );
        }
        assert_eq!(
            classify_local_failover(&policy, LocalFailoverInput::new(500, None)),
            LocalFailoverClassification::RetryUpstreamFailure
        );
    }

    #[test]
    fn classifier_explicit_continue_rule_overrides_protocol_client_error_default() {
        let policy = LocalFailoverPolicy {
            continue_status_codes: [429].into_iter().collect(),
            retry_client_errors_by_default: false,
            ..LocalFailoverPolicy::default()
        };

        assert_eq!(
            classify_local_failover(&policy, LocalFailoverInput::new(429, None)),
            LocalFailoverClassification::RetryStatusCode
        );
    }

    #[test]
    fn classifier_keeps_embedded_rate_limit_error_in_success_response_on_default_path() {
        assert_eq!(
            classify_local_failover(
                &LocalFailoverPolicy::default(),
                LocalFailoverInput::new(
                    200,
                    Some(
                        "{\"error\":{\"message\":\"quota reached\",\"type\":\"rate_limit_error\"}}"
                    )
                )
            ),
            LocalFailoverClassification::UseDefault
        );
    }

    #[test]
    fn legacy_classification_preserves_candidate_by_candidate_retry() {
        assert_eq!(
            failure_disposition_from_local_classification(
                LocalFailoverClassification::RetryUpstreamFailure,
                429,
            ),
            FailureDisposition {
                retry_action: FailureRetryAction::NextCandidate,
                failure_scope: FailureScope::None,
                token_action: FailureTokenAction::None,
                preserve_upstream_error: false,
            }
        );
        assert_eq!(
            failure_disposition_from_local_classification(
                LocalFailoverClassification::StopErrorPattern,
                400,
            )
            .retry_action,
            FailureRetryAction::Stop
        );
    }

    #[test]
    fn anthropic_bad_request_stops_and_preserves_upstream_error() {
        let disposition = classify_anthropic_failure_disposition(
            LocalFailoverClassification::RetryUpstreamFailure,
            400,
        );

        assert_eq!(disposition.retry_action, FailureRetryAction::Stop);
        assert_eq!(disposition.failure_scope, FailureScope::None);
        assert_eq!(disposition.token_action, FailureTokenAction::None);
        assert!(disposition.preserve_upstream_error);
    }

    #[test]
    fn anthropic_auth_failures_refresh_then_rotate_only_when_needed() {
        let unauthorized = classify_anthropic_failure_disposition(
            LocalFailoverClassification::RetryUpstreamFailure,
            401,
        );
        assert_eq!(
            unauthorized.retry_action,
            FailureRetryAction::NextCredential
        );
        assert_eq!(unauthorized.failure_scope, FailureScope::Credential);
        assert_eq!(unauthorized.token_action, FailureTokenAction::ForceRefresh);

        let forbidden = classify_anthropic_failure_disposition(
            LocalFailoverClassification::RetryUpstreamFailure,
            403,
        );
        assert_eq!(forbidden.retry_action, FailureRetryAction::NextCredential);
        assert_eq!(forbidden.failure_scope, FailureScope::Credential);
        assert_eq!(forbidden.token_action, FailureTokenAction::None);
    }

    #[test]
    fn anthropic_rate_limit_rotates_with_credential_model_scope() {
        let disposition = classify_anthropic_failure_disposition(
            LocalFailoverClassification::RetryUpstreamFailure,
            429,
        );

        assert_eq!(disposition.retry_action, FailureRetryAction::NextCredential);
        assert_eq!(disposition.failure_scope, FailureScope::CredentialModel);
        assert!(disposition.failure_scope.affects_credential());
        assert!(!disposition.failure_scope.allows_key_wide_effects());
        assert!(disposition.preserve_upstream_error);
    }

    #[test]
    fn anthropic_overload_moves_endpoint_without_credential_penalty() {
        let disposition = classify_anthropic_failure_disposition(
            LocalFailoverClassification::RetryUpstreamFailure,
            529,
        );

        assert_eq!(disposition.retry_action, FailureRetryAction::NextEndpoint);
        assert_eq!(disposition.failure_scope, FailureScope::Provider);
        assert!(!disposition.failure_scope.affects_credential());
        assert!(!disposition.failure_scope.allows_key_wide_effects());
        assert_eq!(disposition.token_action, FailureTokenAction::None);
        assert!(disposition.preserve_upstream_error);
    }

    #[test]
    fn anthropic_not_found_moves_endpoint_and_oversize_stops() {
        let not_found = classify_anthropic_failure_disposition(
            LocalFailoverClassification::RetryUpstreamFailure,
            404,
        );
        assert_eq!(not_found.retry_action, FailureRetryAction::NextEndpoint);
        assert_eq!(not_found.failure_scope, FailureScope::Endpoint);
        assert!(not_found.preserve_upstream_error);

        let oversized = classify_anthropic_failure_disposition(
            LocalFailoverClassification::RetryUpstreamFailure,
            413,
        );
        assert_eq!(oversized.retry_action, FailureRetryAction::Stop);
        assert_eq!(oversized.failure_scope, FailureScope::None);
        assert!(oversized.preserve_upstream_error);
    }

    #[test]
    fn only_unscoped_and_credential_failures_allow_key_wide_effects() {
        assert!(FailureScope::None.allows_key_wide_effects());
        assert!(FailureScope::Credential.allows_key_wide_effects());
        assert!(!FailureScope::CredentialModel.allows_key_wide_effects());
        assert!(!FailureScope::Endpoint.allows_key_wide_effects());
        assert!(!FailureScope::Provider.allows_key_wide_effects());
    }

    #[test]
    fn anthropic_explicit_stop_keeps_failure_resource_scope() {
        let auth = classify_anthropic_failure_disposition(
            LocalFailoverClassification::StopStatusCode,
            401,
        );
        assert_eq!(auth.retry_action, FailureRetryAction::Stop);
        assert_eq!(auth.failure_scope, FailureScope::Credential);
        assert_eq!(auth.token_action, FailureTokenAction::ForceRefresh);

        let overloaded = classify_anthropic_failure_disposition(
            LocalFailoverClassification::StopStatusCode,
            529,
        );
        assert_eq!(overloaded.retry_action, FailureRetryAction::Stop);
        assert_eq!(overloaded.failure_scope, FailureScope::Provider);
    }
}
