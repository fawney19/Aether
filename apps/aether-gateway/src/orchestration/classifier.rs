use regex::Regex;
use serde_json::Value;

use super::{LocalFailoverPolicy, LocalFailoverRegexRule};

const CLIENT_ERROR_TYPES: &[&str] = &[
    "invalid_request_error",
    "invalid_argument",
    "failed_precondition",
    "validation_error",
    "bad_request",
];

const CLIENT_ERROR_REASONS: &[&str] = &[
    "CONTENT_LENGTH_EXCEEDS_THRESHOLD",
    "CONTEXT_LENGTH_EXCEEDED",
    "MAX_TOKENS_EXCEEDED",
    "INVALID_CONTENT",
    "CONTENT_POLICY_VIOLATION",
];

const CLIENT_ERROR_PATTERNS: &[&str] = &[
    "could not process image",
    "image too large",
    "invalid image",
    "unsupported image",
    "content_policy_violation",
    "context_length_exceeded",
    "content_length_limit",
    "content_length_exceeds",
    "invalid_prompt",
    "content too long",
    "input is too long",
    "message is too long",
    "prompt is too long",
    "image exceeds",
    "pdf too large",
    "file too large",
    "tool_use_id",
    "validationexception",
];

const STRICT_CLIENT_ERROR_PATTERNS: &[&str] =
    &["unknown parameter", "invalid model for this endpoint"];

const COMPATIBILITY_ERROR_PATTERNS: &[&str] = &[
    "unsupported parameter",
    "unsupported model",
    "unsupported feature",
    "not supported with this model",
    "model does not support",
    "parameter is not supported",
    "feature is not supported",
    "not available for this model",
];

const THINKING_ERROR_PATTERNS: &[&str] = &[
    "invalid `signature` in `thinking` block",
    "invalid signature in thinking block",
    "thinking.signature: field required",
    "thinking.signature:",
    "signature verification failed",
    "must start with a thinking block",
    "expected thinking or redacted_thinking",
    "expected `thinking`",
    "expected thinking, found",
    "expected `thinking`, found",
    "expected redacted_thinking, found",
    "expected `redacted_thinking`, found",
    "thoughtsignature",
    "thought_signature",
];

const RETRYABLE_RATE_LIMIT_PATTERNS: &[&str] = &[
    "rate_limit",
    "rate limited",
    "resource_exhausted",
    "throttl",
    "too many requests",
    "quota reached",
    "quota exceeded",
    "quota hit",
];

const RETRYABLE_ACCOUNT_OR_BILLING_PATTERNS: &[&str] = &[
    "organization has been disabled",
    "organization_disabled",
    "account has been disabled",
    "account_disabled",
    "account has been deactivated",
    "account_deactivated",
    "account deactivated",
    "account suspended",
    "account banned",
    "subscription inactive",
    "payment_required",
    "payment required",
    "insufficient_quota",
    "insufficient quota",
    "quota exhausted",
    "credits exhausted",
    "credit balance",
    "credit limit",
    "verify your account",
    "account verification",
    "verification required",
];

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ParsedLocalErrorResponse {
    type_name: Option<String>,
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
    StopSemanticClientError,
    RetrySuccessPattern,
    RetrySemanticCompatibilityError,
    RetrySemanticRateLimit,
    RetrySemanticThinkingError,
    RetryStatusCode,
    RetryUpstreamFailure,
}

impl LocalFailoverClassification {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::UseDefault => "use_default",
            Self::StopStatusCode => "stop_status_code",
            Self::StopErrorPattern => "stop_error_pattern",
            Self::StopSemanticClientError => "stop_semantic_client_error",
            Self::RetrySuccessPattern => "retry_success_pattern",
            Self::RetrySemanticCompatibilityError => "retry_semantic_compatibility_error",
            Self::RetrySemanticRateLimit => "retry_semantic_rate_limit",
            Self::RetrySemanticThinkingError => "retry_semantic_thinking_error",
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

    if input.status_code >= 400
        && input.response_text.is_some_and(|text| {
            policy
                .error_stop_patterns
                .iter()
                .any(|rule| local_failover_regex_rule_matches(rule, text, input.status_code))
        })
    {
        return LocalFailoverClassification::StopErrorPattern;
    }

    if input.status_code == 200
        && input.response_text.is_some_and(|text| {
            policy
                .success_failover_patterns
                .iter()
                .any(|rule| local_failover_regex_rule_matches(rule, text, input.status_code))
        })
    {
        return LocalFailoverClassification::RetrySuccessPattern;
    }

    let parsed_error = parse_local_error_response(input.response_text);

    if is_semantic_thinking_error(input.status_code, &parsed_error) {
        return LocalFailoverClassification::RetrySemanticThinkingError;
    }

    if is_strict_semantic_client_error(input.status_code, &parsed_error) {
        return LocalFailoverClassification::StopSemanticClientError;
    }

    if is_semantic_compatibility_error(input.status_code, &parsed_error) {
        return LocalFailoverClassification::RetrySemanticCompatibilityError;
    }

    if is_semantic_rate_limit_error(input.status_code, &parsed_error) {
        return LocalFailoverClassification::RetrySemanticRateLimit;
    }

    if is_semantic_account_or_billing_error(input.status_code, &parsed_error) {
        return LocalFailoverClassification::RetryUpstreamFailure;
    }

    if is_semantic_client_error(input.status_code, &parsed_error) {
        return LocalFailoverClassification::StopSemanticClientError;
    }

    if policy.continue_status_codes.contains(&input.status_code) {
        return LocalFailoverClassification::RetryStatusCode;
    }

    if should_failover_local_upstream_status(input.status_code) {
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

fn should_failover_local_upstream_status(status_code: u16) -> bool {
    status_code >= 400
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

    parsed.type_name = first_non_empty_json_text(error_object, &["type", "__type"])
        .or_else(|| first_non_empty_json_text(body_object, &["type", "__type"]));
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
    parsed.type_name = parsed
        .type_name
        .or_else(|| first_non_empty_json_text(nested_error_object, &["type", "__type"]))
        .or_else(|| first_non_empty_json_text(nested_object, &["type", "__type"]));
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

fn semantic_search_text(parsed: &ParsedLocalErrorResponse) -> String {
    [
        parsed.type_name.as_deref(),
        parsed.reason.as_deref(),
        parsed.message.as_deref(),
        parsed.raw.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .filter(|value| !value.is_empty())
    .map(str::to_ascii_lowercase)
    .collect::<Vec<_>>()
    .join(" ")
}

fn is_semantic_client_error(status_code: u16, parsed: &ParsedLocalErrorResponse) -> bool {
    if status_code < 400 {
        return false;
    }

    if parsed.type_name.as_deref().is_some_and(|type_name| {
        let type_name = type_name.to_ascii_lowercase();
        CLIENT_ERROR_TYPES
            .iter()
            .any(|pattern| type_name.contains(pattern))
    }) {
        return true;
    }

    if parsed.reason.as_deref().is_some_and(|reason| {
        let reason = reason.to_ascii_uppercase();
        CLIENT_ERROR_REASONS
            .iter()
            .any(|pattern| reason.contains(pattern))
    }) {
        return true;
    }

    let search_text = semantic_search_text(parsed);
    !search_text.is_empty()
        && CLIENT_ERROR_PATTERNS
            .iter()
            .any(|pattern| search_text.contains(&pattern.to_ascii_lowercase()))
}

fn is_strict_semantic_client_error(status_code: u16, parsed: &ParsedLocalErrorResponse) -> bool {
    if status_code < 400 {
        return false;
    }

    let search_text = semantic_search_text(parsed);
    !search_text.is_empty()
        && STRICT_CLIENT_ERROR_PATTERNS
            .iter()
            .any(|pattern| search_text.contains(&pattern.to_ascii_lowercase()))
}

fn is_semantic_compatibility_error(status_code: u16, parsed: &ParsedLocalErrorResponse) -> bool {
    if status_code < 400 {
        return false;
    }

    let search_text = semantic_search_text(parsed);
    !search_text.is_empty()
        && COMPATIBILITY_ERROR_PATTERNS
            .iter()
            .any(|pattern| search_text.contains(&pattern.to_ascii_lowercase()))
}

fn is_semantic_thinking_error(status_code: u16, parsed: &ParsedLocalErrorResponse) -> bool {
    if status_code != 400 {
        return false;
    }

    let search_text = semantic_search_text(parsed);
    !search_text.is_empty()
        && THINKING_ERROR_PATTERNS
            .iter()
            .any(|pattern| search_text.contains(&pattern.to_ascii_lowercase()))
}

fn is_semantic_rate_limit_error(status_code: u16, parsed: &ParsedLocalErrorResponse) -> bool {
    if status_code < 400 {
        return false;
    }

    let search_text = semantic_search_text(parsed);
    !search_text.is_empty()
        && RETRYABLE_RATE_LIMIT_PATTERNS
            .iter()
            .any(|pattern| search_text.contains(&pattern.to_ascii_lowercase()))
}

fn is_semantic_account_or_billing_error(
    status_code: u16,
    parsed: &ParsedLocalErrorResponse,
) -> bool {
    if status_code < 400 {
        return false;
    }

    let search_text = semantic_search_text(parsed);
    !search_text.is_empty()
        && RETRYABLE_ACCOUNT_OR_BILLING_PATTERNS
            .iter()
            .any(|pattern| search_text.contains(&pattern.to_ascii_lowercase()))
}

fn local_failover_regex_rule_matches(
    rule: &LocalFailoverRegexRule,
    response_text: &str,
    status_code: u16,
) -> bool {
    if !rule.status_codes.is_empty() && !rule.status_codes.contains(&status_code) {
        return false;
    }

    Regex::new(&rule.pattern)
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
    fn classifier_stops_semantic_client_errors_without_custom_rule() {
        assert_eq!(
            classify_local_failover(
                &LocalFailoverPolicy::default(),
                LocalFailoverInput::new(
                    400,
                    Some(
                        "{\"error\":{\"type\":\"invalid_request_error\",\"message\":\"prompt is too long\"}}"
                    )
                )
            ),
            LocalFailoverClassification::StopSemanticClientError
        );
    }

    #[test]
    fn classifier_retries_semantic_compatibility_errors() {
        assert_eq!(
            classify_local_failover(
                &LocalFailoverPolicy::default(),
                LocalFailoverInput::new(
                    400,
                    Some("{\"error\":{\"message\":\"Unsupported parameter: max_tokens is not supported with this model\"}}")
                )
            ),
            LocalFailoverClassification::RetrySemanticCompatibilityError
        );
    }

    #[test]
    fn classifier_stops_unknown_parameter_errors_before_compatibility_retry() {
        assert_eq!(
            classify_local_failover(
                &LocalFailoverPolicy::default(),
                LocalFailoverInput::new(
                    400,
                    Some("{\"error\":{\"message\":\"Unknown parameter: 'tools[0].n'.\"}}")
                )
            ),
            LocalFailoverClassification::StopSemanticClientError
        );
    }

    #[test]
    fn classifier_stops_invalid_model_for_endpoint_errors_before_compatibility_retry() {
        assert_eq!(
            classify_local_failover(
                &LocalFailoverPolicy::default(),
                LocalFailoverInput::new(
                    400,
                    Some("{\"error\":{\"message\":\"invalid model for this endpoint\"}}")
                )
            ),
            LocalFailoverClassification::StopSemanticClientError
        );
    }

    #[test]
    fn classifier_retries_semantic_thinking_errors() {
        assert_eq!(
            classify_local_failover(
                &LocalFailoverPolicy::default(),
                LocalFailoverInput::new(
                    400,
                    Some(
                        "{\"error\":{\"message\":\"invalid `signature` in `thinking` block: signature is for a different request\"}}"
                    )
                )
            ),
            LocalFailoverClassification::RetrySemanticThinkingError
        );
    }

    #[test]
    fn classifier_retries_semantic_rate_limit_errors_even_when_status_is_not_429() {
        assert_eq!(
            classify_local_failover(
                &LocalFailoverPolicy::default(),
                LocalFailoverInput::new(
                    400,
                    Some("{\"error\":{\"message\":\"resource_exhausted: quota reached\"}}")
                )
            ),
            LocalFailoverClassification::RetrySemanticRateLimit
        );
    }

    #[test]
    fn classifier_retries_account_and_billing_errors_before_client_error_stop() {
        assert_eq!(
            classify_local_failover(
                &LocalFailoverPolicy::default(),
                LocalFailoverInput::new(
                    403,
                    Some(
                        "{\"error\":{\"type\":\"invalid_request_error\",\"message\":\"verify your account before continuing\"}}"
                    )
                )
            ),
            LocalFailoverClassification::RetryUpstreamFailure
        );

        assert_eq!(
            classify_local_failover(
                &LocalFailoverPolicy::default(),
                LocalFailoverInput::new(
                    402,
                    Some(
                        "{\"error\":{\"type\":\"invalid_request_error\",\"message\":\"payment required: credit balance exhausted\"}}"
                    )
                )
            ),
            LocalFailoverClassification::RetryUpstreamFailure
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
