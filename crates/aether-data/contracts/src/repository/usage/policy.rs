use super::{StoredRequestUsageAudit, UpsertUsageRecord};

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
    pub success_count: i64,
    pub error_count: i64,
    pub total_tokens: i64,
    pub total_cost_usd: f64,
    pub total_response_time_ms: i64,
    pub last_used_at_unix_secs: Option<u64>,
    pub usage_created_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProviderApiKeyUsageDelta {
    pub request_count: i64,
    pub success_count: i64,
    pub error_count: i64,
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
            success_count: after.success_count - before.success_count,
            error_count: after.error_count - before.error_count,
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
            success_count: after.success_count,
            error_count: after.error_count,
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
            success_count: -before.success_count,
            error_count: -before.error_count,
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
            && self.success_count == 0
            && self.error_count == 0
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
    matches!(
        status,
        "completed" | "success" | "ok" | "billed" | "settled"
    ) && status_code.is_none_or(|code| code < 400)
        && error_message.is_none_or(|value| value.trim().is_empty())
}

pub fn provider_api_key_usage_is_error(
    status: &str,
    status_code: Option<u16>,
    error_message: Option<&str>,
) -> bool {
    !matches!(status, "pending" | "streaming")
        && !provider_api_key_usage_is_success(status, status_code, error_message)
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
    let is_in_flight = matches!(usage.status.as_str(), "pending" | "streaming");
    let is_success = provider_api_key_usage_is_success(
        usage.status.as_str(),
        usage.status_code,
        usage.error_message.as_deref(),
    );
    let is_error = provider_api_key_usage_is_error(
        usage.status.as_str(),
        usage.status_code,
        usage.error_message.as_deref(),
    );

    Some(ProviderApiKeyUsageContribution {
        key_id,
        request_count: 1,
        success_count: i64::from(is_success),
        error_count: i64::from(is_error),
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
