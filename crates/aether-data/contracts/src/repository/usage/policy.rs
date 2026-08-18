use super::{StoredRequestUsageAudit, UpsertUsageRecord};

/// Canonical request outcome used by every SLA, success-rate, and error-rate projection.
///
/// HTTP 400 is deliberately modeled as a user error: it remains a terminal request, but it is
/// excluded from the service SLA denominator and from service-error monitoring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestOutcomeClass {
    Success,
    UserError,
    ServiceError,
    Cancelled,
    InFlight,
}

impl RequestOutcomeClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::UserError => "user_error",
            Self::ServiceError => "service_error",
            Self::Cancelled => "cancelled",
            Self::InFlight => "in_flight",
        }
    }

    pub const fn is_sla_eligible(self) -> bool {
        matches!(self, Self::Success | Self::ServiceError)
    }

    pub const fn is_success(self) -> bool {
        matches!(self, Self::Success)
    }

    pub const fn is_user_error(self) -> bool {
        matches!(self, Self::UserError)
    }

    pub const fn is_service_error(self) -> bool {
        matches!(self, Self::ServiceError)
    }
}

impl std::fmt::Display for RequestOutcomeClass {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

pub fn request_outcome_is_success(
    status: &str,
    status_code: Option<u16>,
    error_message: Option<&str>,
) -> bool {
    let status = status.trim().to_ascii_lowercase();
    matches!(
        status.as_str(),
        "completed" | "success" | "ok" | "billed" | "settled"
    ) && status_code.is_none_or(|code| code < 400)
        && error_message.is_none_or(|value| value.trim().is_empty())
}

pub fn classify_request_outcome(
    status: &str,
    status_code: Option<u16>,
    error_message: Option<&str>,
) -> RequestOutcomeClass {
    let status = status.trim().to_ascii_lowercase();
    // HTTP 400 is always a caller/user error, even when a legacy lifecycle
    // status or error message says otherwise.
    if status_code == Some(400) {
        return RequestOutcomeClass::UserError;
    }
    if matches!(status.as_str(), "pending" | "streaming") {
        return RequestOutcomeClass::InFlight;
    }
    if status == "cancelled" {
        return RequestOutcomeClass::Cancelled;
    }
    if request_outcome_is_success(status.as_str(), status_code, error_message) {
        RequestOutcomeClass::Success
    } else {
        RequestOutcomeClass::ServiceError
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ApiKeyUsageContribution {
    pub api_key_id: String,
    pub total_requests: i64,
    pub total_tokens: i64,
    pub total_cost_usd: f64,
    pub last_used_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ApiKeyUsageDelta {
    pub total_requests: i64,
    pub total_tokens: i64,
    pub total_cost_usd: f64,
    pub candidate_last_used_at_unix_secs: Option<u64>,
    pub removed_last_used_at_unix_secs: Option<u64>,
}

impl ApiKeyUsageDelta {
    pub fn between(before: &ApiKeyUsageContribution, after: &ApiKeyUsageContribution) -> Self {
        Self {
            total_requests: after.total_requests - before.total_requests,
            total_tokens: after.total_tokens - before.total_tokens,
            total_cost_usd: after.total_cost_usd - before.total_cost_usd,
            candidate_last_used_at_unix_secs: newer_last_used_at(
                before.last_used_at_unix_secs,
                after.last_used_at_unix_secs,
            ),
            removed_last_used_at_unix_secs: None,
        }
    }

    pub fn addition(after: &ApiKeyUsageContribution) -> Self {
        Self {
            total_requests: after.total_requests,
            total_tokens: after.total_tokens,
            total_cost_usd: after.total_cost_usd,
            candidate_last_used_at_unix_secs: after.last_used_at_unix_secs,
            removed_last_used_at_unix_secs: None,
        }
    }

    pub fn removal(before: &ApiKeyUsageContribution) -> Self {
        Self {
            total_requests: -before.total_requests,
            total_tokens: -before.total_tokens,
            total_cost_usd: -before.total_cost_usd,
            candidate_last_used_at_unix_secs: None,
            removed_last_used_at_unix_secs: before.last_used_at_unix_secs,
        }
    }

    pub fn is_noop(&self) -> bool {
        self.total_requests == 0
            && self.total_tokens == 0
            && self.total_cost_usd == 0.0
            && self.candidate_last_used_at_unix_secs.is_none()
            && self.removed_last_used_at_unix_secs.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelUsageContribution {
    pub model: String,
    pub request_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelUsageDelta {
    pub request_count: i64,
}

impl ModelUsageDelta {
    pub fn between(before: &ModelUsageContribution, after: &ModelUsageContribution) -> Self {
        Self {
            request_count: after.request_count - before.request_count,
        }
    }

    pub fn addition(after: &ModelUsageContribution) -> Self {
        Self {
            request_count: after.request_count,
        }
    }

    pub fn removal(before: &ModelUsageContribution) -> Self {
        Self {
            request_count: -before.request_count,
        }
    }

    pub fn is_noop(&self) -> bool {
        self.request_count == 0
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProviderApiKeyUsageContribution {
    pub key_id: String,
    pub request_count: i64,
    pub sla_eligible_count: i64,
    pub success_count: i64,
    /// Service errors only. HTTP 400 is tracked separately in `user_error_count`.
    pub error_count: i64,
    pub user_error_count: i64,
    pub total_tokens: i64,
    pub total_cost_usd: f64,
    pub total_response_time_ms: i64,
    pub last_used_at_unix_secs: Option<u64>,
    pub usage_created_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProviderApiKeyUsageDelta {
    pub request_count: i64,
    pub sla_eligible_count: i64,
    pub success_count: i64,
    /// Service errors only. HTTP 400 is tracked separately in `user_error_count`.
    pub error_count: i64,
    pub user_error_count: i64,
    pub total_tokens: i64,
    pub total_cost_usd: f64,
    pub total_response_time_ms: i64,
    pub candidate_last_used_at_unix_secs: Option<u64>,
    pub removed_last_used_at_unix_secs: Option<u64>,
    pub usage_created_at_unix_secs: Option<u64>,
}

impl ProviderApiKeyUsageDelta {
    pub fn between(
        before: &ProviderApiKeyUsageContribution,
        after: &ProviderApiKeyUsageContribution,
    ) -> Self {
        Self {
            request_count: after.request_count - before.request_count,
            sla_eligible_count: after.sla_eligible_count - before.sla_eligible_count,
            success_count: after.success_count - before.success_count,
            error_count: after.error_count - before.error_count,
            user_error_count: after.user_error_count - before.user_error_count,
            total_tokens: after.total_tokens - before.total_tokens,
            total_cost_usd: after.total_cost_usd - before.total_cost_usd,
            total_response_time_ms: after.total_response_time_ms - before.total_response_time_ms,
            candidate_last_used_at_unix_secs: newer_last_used_at(
                before.last_used_at_unix_secs,
                after.last_used_at_unix_secs,
            ),
            removed_last_used_at_unix_secs: None,
            usage_created_at_unix_secs: after.usage_created_at_unix_secs,
        }
    }

    pub fn addition(after: &ProviderApiKeyUsageContribution) -> Self {
        Self {
            request_count: after.request_count,
            sla_eligible_count: after.sla_eligible_count,
            success_count: after.success_count,
            error_count: after.error_count,
            user_error_count: after.user_error_count,
            total_tokens: after.total_tokens,
            total_cost_usd: after.total_cost_usd,
            total_response_time_ms: after.total_response_time_ms,
            candidate_last_used_at_unix_secs: after.last_used_at_unix_secs,
            removed_last_used_at_unix_secs: None,
            usage_created_at_unix_secs: after.usage_created_at_unix_secs,
        }
    }

    pub fn removal(before: &ProviderApiKeyUsageContribution) -> Self {
        Self {
            request_count: -before.request_count,
            sla_eligible_count: -before.sla_eligible_count,
            success_count: -before.success_count,
            error_count: -before.error_count,
            user_error_count: -before.user_error_count,
            total_tokens: -before.total_tokens,
            total_cost_usd: -before.total_cost_usd,
            total_response_time_ms: -before.total_response_time_ms,
            candidate_last_used_at_unix_secs: None,
            removed_last_used_at_unix_secs: before.last_used_at_unix_secs,
            usage_created_at_unix_secs: before.usage_created_at_unix_secs,
        }
    }

    pub fn is_noop(&self) -> bool {
        self.request_count == 0
            && self.sla_eligible_count == 0
            && self.success_count == 0
            && self.error_count == 0
            && self.user_error_count == 0
            && self.total_tokens == 0
            && self.total_cost_usd == 0.0
            && self.total_response_time_ms == 0
            && self.candidate_last_used_at_unix_secs.is_none()
            && self.removed_last_used_at_unix_secs.is_none()
    }
}

pub fn incoming_usage_can_recover_terminal_failure(
    incoming_status: &str,
    incoming_billing_status: &str,
) -> bool {
    incoming_billing_status == "pending" && incoming_status == "completed"
}

pub fn usage_can_recover_terminal_failure(
    existing_status: &str,
    existing_billing_status: &str,
    incoming_status: &str,
    incoming_billing_status: &str,
) -> bool {
    existing_billing_status == "void"
        && matches!(existing_status, "failed" | "cancelled")
        && incoming_usage_can_recover_terminal_failure(incoming_status, incoming_billing_status)
}

pub fn strip_deprecated_usage_display_fields(mut usage: UpsertUsageRecord) -> UpsertUsageRecord {
    usage.username = None;
    usage.api_key_name = None;
    usage
}

pub fn provider_api_key_usage_is_success(
    status: &str,
    status_code: Option<u16>,
    error_message: Option<&str>,
) -> bool {
    classify_request_outcome(status, status_code, error_message).is_success()
}

pub fn provider_api_key_usage_is_error(
    status: &str,
    status_code: Option<u16>,
    error_message: Option<&str>,
) -> bool {
    classify_request_outcome(status, status_code, error_message).is_service_error()
}

pub fn provider_api_key_usage_is_user_error(
    status: &str,
    status_code: Option<u16>,
    error_message: Option<&str>,
) -> bool {
    classify_request_outcome(status, status_code, error_message).is_user_error()
}

pub fn provider_api_key_usage_is_sla_eligible(
    status: &str,
    status_code: Option<u16>,
    error_message: Option<&str>,
) -> bool {
    classify_request_outcome(status, status_code, error_message).is_sla_eligible()
}

pub fn provider_api_key_usage_contribution(
    usage: &StoredRequestUsageAudit,
) -> Option<ProviderApiKeyUsageContribution> {
    let key_id = usage
        .provider_api_key_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    let outcome = classify_request_outcome(
        usage.status.as_str(),
        usage.status_code,
        usage.error_message.as_deref(),
    );
    let is_in_flight = matches!(outcome, RequestOutcomeClass::InFlight);
    let is_success = outcome.is_success();

    Some(ProviderApiKeyUsageContribution {
        key_id,
        request_count: 1,
        sla_eligible_count: i64::from(outcome.is_sla_eligible()),
        success_count: i64::from(is_success),
        error_count: i64::from(outcome.is_service_error()),
        user_error_count: i64::from(outcome.is_user_error()),
        total_tokens: if is_in_flight {
            0
        } else {
            i64::try_from(usage.total_tokens).unwrap_or(i64::MAX)
        },
        total_cost_usd: if is_in_flight {
            0.0
        } else if usage.total_cost_usd.is_finite() {
            usage.total_cost_usd.max(0.0)
        } else {
            0.0
        },
        total_response_time_ms: if is_success {
            usage
                .response_time_ms
                .and_then(|value| i64::try_from(value).ok())
                .unwrap_or_default()
        } else {
            0
        },
        last_used_at_unix_secs: Some(usage.created_at_unix_ms),
        usage_created_at_unix_secs: Some(usage.created_at_unix_ms),
    })
}

pub fn model_usage_contribution(usage: &StoredRequestUsageAudit) -> Option<ModelUsageContribution> {
    if matches!(usage.status.as_str(), "pending" | "streaming") {
        return None;
    }
    let model = usage.model.trim();
    if model.is_empty() {
        return None;
    }
    Some(ModelUsageContribution {
        model: model.to_string(),
        request_count: 1,
    })
}

pub fn api_key_usage_contribution(
    usage: &StoredRequestUsageAudit,
) -> Option<ApiKeyUsageContribution> {
    if matches!(usage.status.as_str(), "pending" | "streaming") {
        return None;
    }
    let api_key_id = usage
        .api_key_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    Some(ApiKeyUsageContribution {
        api_key_id,
        total_requests: 1,
        total_tokens: i64::try_from(usage.total_tokens).unwrap_or(i64::MAX),
        total_cost_usd: if usage.total_cost_usd.is_finite() {
            usage.total_cost_usd.max(0.0)
        } else {
            0.0
        },
        last_used_at_unix_secs: Some(usage.created_at_unix_ms),
    })
}

fn newer_last_used_at(before: Option<u64>, after: Option<u64>) -> Option<u64> {
    match (before, after) {
        (Some(before), Some(after)) if after > before => Some(after),
        (None, Some(after)) => Some(after),
        _ => None,
    }
}

#[cfg(test)]
mod outcome_tests {
    use super::{classify_request_outcome, RequestOutcomeClass};

    #[test]
    fn exact_http_400_is_a_user_error_even_with_failure_signals() {
        let outcome = classify_request_outcome("failed", Some(400), Some("invalid request"));

        assert_eq!(outcome, RequestOutcomeClass::UserError);
        assert!(!outcome.is_sla_eligible());
        assert!(!outcome.is_service_error());
    }

    #[test]
    fn exact_http_400_wins_over_legacy_cancelled_status() {
        assert_eq!(
            classify_request_outcome("cancelled", Some(400), Some("invalid request")),
            RequestOutcomeClass::UserError
        );
        assert_eq!(
            classify_request_outcome("pending", Some(400), Some("invalid request")),
            RequestOutcomeClass::UserError
        );
    }

    #[test]
    fn non_400_terminal_failures_are_service_errors() {
        for status_code in [None, Some(401), Some(422), Some(429), Some(500)] {
            let outcome = classify_request_outcome("failed", status_code, Some("upstream error"));

            assert_eq!(outcome, RequestOutcomeClass::ServiceError);
            assert!(outcome.is_sla_eligible());
        }
    }

    #[test]
    fn successes_are_sla_eligible_and_non_terminal_requests_are_not() {
        let success = classify_request_outcome("completed", Some(200), None);
        let pending = classify_request_outcome("pending", None, None);
        let cancelled = classify_request_outcome("cancelled", Some(499), None);

        assert_eq!(success, RequestOutcomeClass::Success);
        assert!(success.is_sla_eligible());
        assert_eq!(pending, RequestOutcomeClass::InFlight);
        assert!(!pending.is_sla_eligible());
        assert_eq!(cancelled, RequestOutcomeClass::Cancelled);
        assert!(!cancelled.is_sla_eligible());
    }

    #[test]
    fn classification_normalizes_legacy_status_casing_and_whitespace() {
        assert_eq!(
            classify_request_outcome("  COMPLETED ", Some(200), None),
            RequestOutcomeClass::Success
        );
        assert_eq!(
            classify_request_outcome(" FAILED ", Some(400), Some("bad input")),
            RequestOutcomeClass::UserError
        );
    }
}
