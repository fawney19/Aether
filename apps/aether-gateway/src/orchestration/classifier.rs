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

pub(crate) fn classify_local_failover(
    policy: &LocalFailoverPolicy,
    input: LocalFailoverInput<'_>,
) -> LocalFailoverClassification {
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
    if !rule.status_codes.is_empty() && !rule.status_codes.contains(&status_code) {
        return false;
    }

    let pattern = rule.pattern.trim();
    if pattern.is_empty() {
        return !rule.status_codes.is_empty();
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
    use std::collections::BTreeSet;

    use super::{classify_local_failover, LocalFailoverClassification, LocalFailoverInput};
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
}
