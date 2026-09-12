use super::{StoredRequestUsageAudit, UpsertUsageRecord};
use serde_json::{Map, Value};

const MAX_USAGE_CANDIDATE_INDEX: u64 = i32::MAX as u64;
const MAX_USAGE_CANDIDATE_ID_LEN: usize = 128;
const MAX_USAGE_KEY_NAME_LEN: usize = 255;
const MAX_USAGE_PLANNER_KIND_LEN: usize = 64;
const MAX_USAGE_ROUTE_FAMILY_LEN: usize = 80;
const MAX_USAGE_ROUTE_KIND_LEN: usize = 80;
const MAX_USAGE_EXECUTION_PATH_LEN: usize = 80;
const MAX_USAGE_RUNTIME_MISS_REASON_LEN: usize = 120;

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

/// Decide whether an incoming lifecycle event may replace an existing usage revision.
///
/// `updated_at_unix_secs` is authoritative. `finalized_at_unix_secs` breaks ties when writers
/// observe multiple transitions in the same second. Equal pending revisions may still progress to
/// streaming or terminal states, while every terminal replay requires a strictly newer revision.
/// The explicit void-failure recovery remains available at an equal revision, but never for an
/// older event.
#[allow(clippy::too_many_arguments)]
pub fn usage_lifecycle_update_allowed(
    existing_status: &str,
    existing_billing_status: &str,
    existing_updated_at_unix_secs: u64,
    existing_finalized_at_unix_secs: Option<u64>,
    incoming_status: &str,
    incoming_billing_status: &str,
    incoming_updated_at_unix_secs: u64,
    incoming_finalized_at_unix_secs: Option<u64>,
) -> bool {
    let existing_revision = (
        existing_updated_at_unix_secs,
        existing_finalized_at_unix_secs.unwrap_or_default(),
    );
    let incoming_revision = (
        incoming_updated_at_unix_secs,
        incoming_finalized_at_unix_secs.unwrap_or_default(),
    );
    if incoming_revision < existing_revision {
        return false;
    }

    let can_recover = usage_can_recover_terminal_failure(
        existing_status,
        existing_billing_status,
        incoming_status,
        incoming_billing_status,
    );
    let existing_is_terminal = matches!(existing_status, "completed" | "failed" | "cancelled");
    let incoming_is_terminal = matches!(incoming_status, "completed" | "failed" | "cancelled");
    if existing_is_terminal && !incoming_is_terminal {
        return false;
    }
    if existing_status == "streaming" && incoming_status == "pending" {
        return false;
    }
    if incoming_revision == existing_revision && existing_is_terminal && incoming_is_terminal {
        return can_recover;
    }

    true
}

pub fn strip_deprecated_usage_display_fields(mut usage: UpsertUsageRecord) -> UpsertUsageRecord {
    usage.username = None;
    usage.api_key_name = None;
    usage
}

fn sanitize_usage_record_metadata(mut usage: UpsertUsageRecord) -> UpsertUsageRecord {
    usage = strip_deprecated_usage_display_fields(usage);
    sanitize_usage_routing_fields(&mut usage, None);
    usage.error_message = None;
    usage.error_category = sanitize_usage_error_category(usage.error_category);
    if usage.error_category.is_none() && usage.status == "failed" {
        usage.error_category = usage
            .status_code
            .map(usage_error_category_for_status_code)
            .map(str::to_string);
    }
    usage.request_metadata = super::sanitize_usage_request_metadata(usage.request_metadata);
    usage
}

pub fn sanitize_usage_for_persistence(usage: UpsertUsageRecord) -> UpsertUsageRecord {
    let mut usage = sanitize_usage_record_metadata(usage);
    usage.request_headers = None;
    usage.request_body = None;
    usage.request_body_ref = None;
    usage.request_body_state = None;
    usage.provider_request_headers = None;
    usage.provider_request_body = None;
    usage.provider_request_body_ref = None;
    usage.provider_request_body_state = None;
    usage.response_headers = None;
    usage.response_body = None;
    usage.response_body_ref = None;
    usage.response_body_state = None;
    usage.client_response_headers = None;
    usage.client_response_body = None;
    usage.client_response_body_ref = None;
    usage.client_response_body_state = None;
    usage
}

pub fn sanitize_usage_capture_controls_for_persistence(
    mut usage: UpsertUsageRecord,
) -> UpsertUsageRecord {
    let metadata = usage
        .request_metadata
        .as_ref()
        .and_then(Value::as_object)
        .cloned();
    sanitize_usage_routing_fields(&mut usage, metadata.as_ref());
    let mut usage = sanitize_usage_record_metadata(usage);
    for headers in [
        &mut usage.request_headers,
        &mut usage.provider_request_headers,
        &mut usage.response_headers,
        &mut usage.client_response_headers,
    ] {
        *headers = sanitize_usage_headers_for_persistence(headers.take());
    }
    for (field, body, body_ref, state) in [
        (
            super::UsageBodyField::RequestBody,
            &mut usage.request_body,
            &mut usage.request_body_ref,
            usage.request_body_state,
        ),
        (
            super::UsageBodyField::ProviderRequestBody,
            &mut usage.provider_request_body,
            &mut usage.provider_request_body_ref,
            usage.provider_request_body_state,
        ),
        (
            super::UsageBodyField::ResponseBody,
            &mut usage.response_body,
            &mut usage.response_body_ref,
            usage.response_body_state,
        ),
        (
            super::UsageBodyField::ClientResponseBody,
            &mut usage.client_response_body,
            &mut usage.client_response_body_ref,
            usage.client_response_body_state,
        ),
    ] {
        if matches!(
            state,
            Some(
                super::UsageBodyCaptureState::None
                    | super::UsageBodyCaptureState::Disabled
                    | super::UsageBodyCaptureState::Unavailable
            )
        ) {
            *body = None;
            *body_ref = None;
        } else {
            *body_ref = body_ref.as_deref().and_then(|value| {
                super::canonical_usage_body_ref_for(value, &usage.request_id, field)
            });
        }
    }
    usage
}

pub fn sanitize_usage_headers_for_persistence(value: Option<Value>) -> Option<Value> {
    value.filter(Value::is_object)
}

fn sanitize_usage_routing_fields(
    usage: &mut UpsertUsageRecord,
    metadata: Option<&Map<String, Value>>,
) {
    usage.candidate_id = sanitize_usage_routing_string_with_metadata(
        usage.candidate_id.take(),
        metadata,
        "candidate_id",
        MAX_USAGE_CANDIDATE_ID_LEN,
        false,
    );
    usage.candidate_index =
        sanitize_usage_routing_index_with_metadata(usage.candidate_index.take(), metadata);
    usage.key_name = sanitize_usage_routing_string_with_metadata(
        usage.key_name.take(),
        metadata,
        "key_name",
        MAX_USAGE_KEY_NAME_LEN,
        true,
    );
    usage.planner_kind = sanitize_usage_routing_string_with_metadata(
        usage.planner_kind.take(),
        metadata,
        "planner_kind",
        MAX_USAGE_PLANNER_KIND_LEN,
        false,
    );
    usage.route_family = sanitize_usage_routing_string_with_metadata(
        usage.route_family.take(),
        metadata,
        "route_family",
        MAX_USAGE_ROUTE_FAMILY_LEN,
        false,
    );
    usage.route_kind = sanitize_usage_routing_string_with_metadata(
        usage.route_kind.take(),
        metadata,
        "route_kind",
        MAX_USAGE_ROUTE_KIND_LEN,
        false,
    );
    usage.execution_path = sanitize_usage_routing_string_with_metadata(
        usage.execution_path.take(),
        metadata,
        "execution_path",
        MAX_USAGE_EXECUTION_PATH_LEN,
        false,
    );
    usage.local_execution_runtime_miss_reason = sanitize_usage_routing_string_with_metadata(
        usage.local_execution_runtime_miss_reason.take(),
        metadata,
        "local_execution_runtime_miss_reason",
        MAX_USAGE_RUNTIME_MISS_REASON_LEN,
        false,
    );
}

fn sanitize_usage_routing_string_with_metadata(
    typed: Option<String>,
    metadata: Option<&Map<String, Value>>,
    key: &str,
    max_len: usize,
    allow_spaces: bool,
) -> Option<String> {
    match typed {
        Some(value) => sanitize_usage_routing_string(Some(value), max_len, allow_spaces),
        None => metadata_routing_string(metadata, key, max_len, allow_spaces),
    }
}

fn sanitize_usage_routing_index_with_metadata(
    typed: Option<u64>,
    metadata: Option<&Map<String, Value>>,
) -> Option<u64> {
    match typed {
        Some(value) => sanitize_usage_routing_index(Some(value)),
        None => metadata
            .and_then(|object| object.get("candidate_index"))
            .and_then(|value| {
                value
                    .as_u64()
                    .or_else(|| value.as_i64().and_then(|value| u64::try_from(value).ok()))
                    .filter(|value| *value <= MAX_USAGE_CANDIDATE_INDEX)
            }),
    }
}

fn metadata_routing_string(
    metadata: Option<&Map<String, Value>>,
    key: &str,
    max_len: usize,
    allow_spaces: bool,
) -> Option<String> {
    metadata
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .and_then(|value| {
            sanitize_usage_routing_string(Some(value.to_string()), max_len, allow_spaces)
        })
}

fn sanitize_usage_routing_index(value: Option<u64>) -> Option<u64> {
    value.filter(|value| *value <= MAX_USAGE_CANDIDATE_INDEX)
}

fn sanitize_usage_routing_string(
    value: Option<String>,
    max_len: usize,
    allow_spaces: bool,
) -> Option<String> {
    let value = value?;
    let value = value.trim();
    if value.is_empty() || value.len() > max_len {
        return None;
    }
    if !value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || b"._:/@+-".contains(&byte) || (allow_spaces && byte == b' ')
    }) {
        return None;
    }
    Some(value.to_string())
}

pub(crate) fn sanitize_usage_error_category(value: Option<String>) -> Option<String> {
    let value = value?.trim().to_ascii_lowercase();
    let category = match value.as_str() {
        "auth"
        | "cancelled"
        | "client_error"
        | "http_error"
        | "non_success_status"
        | "provider_error"
        | "rate_limit"
        | "redirect"
        | "server_error"
        | "stream_missing_terminal_event"
        | "stream_terminal_error"
        | "upstream_error" => value,
        "" => return None,
        _ => "other_error".to_string(),
    };
    Some(category)
}

/// Return the bounded error category represented by an HTTP status code.
///
/// Stale-request cleanup may have only a candidate status code available.  Do
/// not persist provider-supplied diagnostic text in that case; derive one of
/// the same fixed categories used by the usage writer instead.
pub fn usage_error_category_for_status_code(status_code: u16) -> &'static str {
    if status_code >= 500 {
        "server_error"
    } else if status_code >= 400 {
        "client_error"
    } else if status_code >= 300 {
        "redirect"
    } else {
        "non_success_status"
    }
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
        total_tokens: if is_in_flight || !usage.usage_available() {
            0
        } else {
            i64::try_from(usage.total_tokens).unwrap_or(i64::MAX)
        },
        total_cost_usd: if is_in_flight
            || !usage.usage_available()
            || !usage.usage_pricing_available()
        {
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
        total_tokens: if usage.usage_available() {
            i64::try_from(usage.total_tokens).unwrap_or(i64::MAX)
        } else {
            0
        },
        total_cost_usd: if !usage.usage_available() || !usage.usage_pricing_available() {
            0.0
        } else if usage.total_cost_usd.is_finite() {
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
mod tests {
    use serde_json::json;

    use super::{
        sanitize_usage_capture_controls_for_persistence, sanitize_usage_error_category,
        sanitize_usage_for_persistence, usage_error_category_for_status_code,
        usage_lifecycle_update_allowed,
    };
    use crate::repository::usage::{UpsertUsageRecord, UsageBodyCaptureState};

    fn usage_with_http_capture() -> UpsertUsageRecord {
        UpsertUsageRecord {
            capture_retention: Default::default(),
            request_id: "req-sensitive-capture".to_string(),
            user_id: Some("user-1".to_string()),
            api_key_id: Some("key-1".to_string()),
            username: Some("alice".to_string()),
            api_key_name: Some("primary".to_string()),
            provider_name: "OpenAI".to_string(),
            model: "gpt-5".to_string(),
            target_model: None,
            provider_id: Some("provider-1".to_string()),
            provider_endpoint_id: None,
            provider_api_key_id: None,
            request_type: Some("chat".to_string()),
            api_format: Some("openai:chat".to_string()),
            api_family: Some("openai".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_api_format: Some("openai:chat".to_string()),
            provider_api_family: Some("openai".to_string()),
            provider_endpoint_kind: Some("chat".to_string()),
            has_format_conversion: Some(false),
            is_stream: Some(false),
            input_tokens: Some(1),
            output_tokens: Some(2),
            total_tokens: Some(3),
            cache_creation_input_tokens: None,
            cache_creation_ephemeral_5m_input_tokens: None,
            cache_creation_ephemeral_1h_input_tokens: None,
            cache_read_input_tokens: None,
            cache_creation_cost_usd: None,
            cache_read_cost_usd: None,
            output_price_per_1m: None,
            total_cost_usd: Some(0.01),
            actual_total_cost_usd: Some(0.01),
            status_code: Some(200),
            error_message: Some("Bearer secret".to_string()),
            error_category: Some("provider_error".to_string()),
            response_time_ms: Some(10),
            first_byte_time_ms: Some(5),
            status: "completed".to_string(),
            billing_status: "settled".to_string(),
            request_headers: Some(json!({"authorization": "Bearer secret"})),
            request_body: Some(json!({"prompt": "private"})),
            request_body_ref: Some("usage://request/body".to_string()),
            request_body_state: Some(UsageBodyCaptureState::Reference),
            provider_request_headers: Some(json!({"x-api-key": "secret"})),
            provider_request_body: Some(json!({"prompt": "private"})),
            provider_request_body_ref: Some("usage://provider/body".to_string()),
            provider_request_body_state: Some(UsageBodyCaptureState::Inline),
            response_headers: Some(json!({"set-cookie": "secret"})),
            response_body: Some(json!({"output": "private"})),
            response_body_ref: Some("usage://response/body".to_string()),
            response_body_state: Some(UsageBodyCaptureState::Truncated),
            client_response_headers: Some(json!({"x-private": "secret"})),
            client_response_body: Some(json!({"output": "private"})),
            client_response_body_ref: Some("usage://client/body".to_string()),
            client_response_body_state: Some(UsageBodyCaptureState::Disabled),
            candidate_id: Some("candidate-1".to_string()),
            candidate_index: Some(0),
            key_name: None,
            planner_kind: None,
            route_family: None,
            route_kind: None,
            execution_path: None,
            local_execution_runtime_miss_reason: None,
            request_metadata: Some(json!({"client_ip": "203.0.113.8"})),
            finalized_at_unix_secs: Some(2),
            created_at_unix_ms: Some(1_000),
            updated_at_unix_secs: 2,
        }
    }

    #[test]
    fn usage_error_categories_are_bounded_to_controlled_values() {
        assert_eq!(
            sanitize_usage_error_category(Some(" Server_Error ".to_string())).as_deref(),
            Some("server_error")
        );
        assert_eq!(
            sanitize_usage_error_category(Some("Authorization: Bearer secret".to_string()))
                .as_deref(),
            Some("other_error")
        );
        assert_eq!(sanitize_usage_error_category(Some("  ".to_string())), None);
    }

    #[test]
    fn status_codes_map_to_bounded_error_categories() {
        assert_eq!(usage_error_category_for_status_code(599), "server_error");
        assert_eq!(usage_error_category_for_status_code(400), "client_error");
        assert_eq!(usage_error_category_for_status_code(302), "redirect");
        assert_eq!(
            usage_error_category_for_status_code(200),
            "non_success_status"
        );
    }

    #[test]
    fn persistence_derives_missing_failed_category_from_status_code() {
        let mut usage = usage_with_http_capture();
        usage.status = "failed".to_string();
        usage.status_code = Some(429);
        usage.error_category = None;

        let usage = sanitize_usage_for_persistence(usage);
        assert_eq!(usage.error_category.as_deref(), Some("client_error"));
    }

    #[test]
    fn persistence_boundary_drops_all_http_capture_material() {
        let usage = sanitize_usage_for_persistence(usage_with_http_capture());

        assert!(usage.username.is_none());
        assert!(usage.api_key_name.is_none());
        assert!(usage.error_message.is_none());
        assert!(usage.request_headers.is_none());
        assert!(usage.request_body.is_none());
        assert!(usage.request_body_ref.is_none());
        assert!(usage.request_body_state.is_none());
        assert!(usage.provider_request_headers.is_none());
        assert!(usage.provider_request_body.is_none());
        assert!(usage.provider_request_body_ref.is_none());
        assert!(usage.provider_request_body_state.is_none());
        assert!(usage.response_headers.is_none());
        assert!(usage.response_body.is_none());
        assert!(usage.response_body_ref.is_none());
        assert!(usage.response_body_state.is_none());
        assert!(usage.client_response_headers.is_none());
        assert!(usage.client_response_body.is_none());
        assert!(usage.client_response_body_ref.is_none());
        assert!(usage.client_response_body_state.is_none());
        assert_eq!(usage.error_category.as_deref(), Some("provider_error"));
        assert_eq!(usage.candidate_id.as_deref(), Some("candidate-1"));
        assert_eq!(
            usage.request_metadata,
            Some(json!({"client_ip": "203.0.113.8"}))
        );
    }

    #[test]
    fn header_capture_preserves_original_values() {
        let headers = json!({
            "Authorization": "Bearer original-token",
            "originator": "codex-cli",
            "user-agent": "codex-tui/test",
            "session-id": "session-original",
            "thread-id": "thread-original",
            "x-client-request-id": "client-request-original",
            "x-codex-beta-features": "feature-original",
            "x-codex-parent-thread-id": "parent-thread-original",
            "x-codex-turn-metadata": "{\"turn_id\":\"turn-original\"}",
            "x-codex-turn-state": "turn-state-original",
            "x-codex-window-id": "window-original",
            "x-openai-internal-codex-responses-lite": "true",
            "x-openai-subagent": "reviewer",
            "set-cookie": ["session=original", "preference=original"],
            "x-custom-header": "original-value",
            "x-null": null
        });
        assert_eq!(
            super::sanitize_usage_headers_for_persistence(Some(headers.clone())),
            Some(headers)
        );
        for invalid in [json!(null), json!("headers"), json!(["headers"])] {
            assert!(super::sanitize_usage_headers_for_persistence(Some(invalid)).is_none());
        }
    }

    #[test]
    fn auxiliary_capture_projection_preserves_captures_and_honors_disabled_states() {
        let mut input = usage_with_http_capture();
        input.request_body_state = Some(UsageBodyCaptureState::None);
        input.response_body_state = Some(UsageBodyCaptureState::Disabled);

        let usage = sanitize_usage_capture_controls_for_persistence(input);

        assert_eq!(
            usage.request_headers,
            Some(json!({"authorization": "Bearer secret"}))
        );
        assert!(usage.request_body.is_none());
        assert!(usage.request_body_ref.is_none());
        assert_eq!(usage.request_body_state, Some(UsageBodyCaptureState::None));
        assert_eq!(
            usage.provider_request_body,
            Some(json!({"prompt": "private"}))
        );
        assert_eq!(
            usage.provider_request_body_state,
            Some(UsageBodyCaptureState::Inline)
        );
        assert!(usage.response_body.is_none());
        assert!(usage.response_body_ref.is_none());
        assert_eq!(
            usage.response_body_state,
            Some(UsageBodyCaptureState::Disabled)
        );
        assert!(usage.client_response_body.is_none());
        assert_eq!(
            usage.client_response_body_state,
            Some(UsageBodyCaptureState::Disabled)
        );
    }

    #[test]
    fn auxiliary_capture_projection_preserves_all_body_directions_and_scopes_references() {
        let mut input = usage_with_http_capture();
        input.request_headers =
            Some(json!({"Content-Type": "application/json", "Authorization": "Bearer secret"}));
        input.request_body_state = Some(UsageBodyCaptureState::Inline);
        input.client_response_body_state = Some(UsageBodyCaptureState::Inline);
        input.request_body_ref = Some(super::super::usage_body_ref(
            &input.request_id,
            super::super::UsageBodyField::RequestBody,
        ));
        input.provider_request_body_ref = input.request_body_ref.clone();
        input.response_body_ref = Some(super::super::usage_body_ref(
            "another-request",
            super::super::UsageBodyField::ResponseBody,
        ));
        let captured = sanitize_usage_capture_controls_for_persistence(input.clone());
        assert_eq!(captured.request_body, input.request_body);
        assert_eq!(captured.provider_request_body, input.provider_request_body);
        assert_eq!(captured.response_body, input.response_body);
        assert_eq!(captured.client_response_body, input.client_response_body);
        assert_eq!(captured.request_body_ref, input.request_body_ref);
        assert!(captured.provider_request_body_ref.is_none());
        assert!(captured.response_body_ref.is_none());
        assert!(captured.client_response_body_ref.is_none());
        assert_eq!(
            captured.request_headers,
            Some(json!({"Content-Type": "application/json", "Authorization": "Bearer secret"}))
        );
        assert_eq!(
            captured.provider_request_headers,
            Some(json!({"x-api-key": "secret"}))
        );
        assert_eq!(
            captured.response_headers,
            Some(json!({"set-cookie": "secret"}))
        );
        assert_eq!(
            captured.client_response_headers,
            input.client_response_headers
        );
        assert!(captured.error_message.is_none());
    }

    #[test]
    fn auxiliary_projection_promotes_only_bounded_routing_metadata() {
        let mut input = usage_with_http_capture();
        input.candidate_id = None;
        input.candidate_index = None;
        input.key_name = None;
        input.planner_kind = None;
        input.route_family = None;
        input.route_kind = None;
        input.execution_path = None;
        input.local_execution_runtime_miss_reason = None;
        input.request_metadata = Some(json!({
            "trace_id": "trace",
            "candidate_id": "candidate-from-metadata",
            "candidate_index": 7,
            "key_name": "primary key",
            "planner_kind": "fallback",
            "route_family": "chat",
            "route_kind": "remote",
            "execution_path": "execution_runtime_stream",
            "local_execution_runtime_miss_reason": "runtime_busy",
            "authorization": "Bearer should-not-persist",
        }));

        let usage = sanitize_usage_capture_controls_for_persistence(input);

        assert_eq!(
            usage.candidate_id.as_deref(),
            Some("candidate-from-metadata")
        );
        assert_eq!(usage.candidate_index, Some(7));
        assert_eq!(usage.key_name.as_deref(), Some("primary key"));
        assert_eq!(usage.planner_kind.as_deref(), Some("fallback"));
        assert_eq!(usage.route_family.as_deref(), Some("chat"));
        assert_eq!(usage.route_kind.as_deref(), Some("remote"));
        assert_eq!(
            usage.execution_path.as_deref(),
            Some("execution_runtime_stream")
        );
        assert_eq!(
            usage.local_execution_runtime_miss_reason.as_deref(),
            Some("runtime_busy")
        );
        assert_eq!(usage.request_metadata, Some(json!({"trace_id": "trace"})));
    }

    #[test]
    fn routing_projection_rejects_unbounded_or_control_character_values() {
        let mut input = usage_with_http_capture();
        input.candidate_id = Some("candidate\nforged".to_string());
        input.candidate_index = Some(u64::MAX);
        input.key_name = Some("key\0name".to_string());
        input.planner_kind = Some("p".repeat(65));
        input.route_family = Some("route\tname".to_string());
        input.route_kind = Some("route-kind".to_string());
        input.execution_path = Some("execution-path".to_string());
        input.local_execution_runtime_miss_reason = Some("m".repeat(121));

        let usage = sanitize_usage_for_persistence(input);

        assert!(usage.candidate_id.is_none());
        assert!(usage.candidate_index.is_none());
        assert!(usage.key_name.is_none());
        assert!(usage.planner_kind.is_none());
        assert!(usage.route_family.is_none());
        assert_eq!(usage.route_kind.as_deref(), Some("route-kind"));
        assert_eq!(usage.execution_path.as_deref(), Some("execution-path"));
        assert!(usage.local_execution_runtime_miss_reason.is_none());
    }

    #[test]
    fn invalid_typed_routing_values_do_not_fall_back_to_metadata() {
        let mut input = usage_with_http_capture();
        input.candidate_id = Some("candidate\nforged".to_string());
        input.candidate_index = Some(u64::MAX);
        input.key_name = Some("key\0name".to_string());
        input.planner_kind = Some("p".repeat(65));
        input.route_family = Some("route\tname".to_string());
        input.route_kind = Some("route\nkind".to_string());
        input.execution_path = Some("execution\npath".to_string());
        input.local_execution_runtime_miss_reason = Some("m".repeat(121));
        input.request_metadata = Some(json!({
            "candidate_id": "metadata-candidate",
            "candidate_index": 7,
            "key_name": "metadata key",
            "planner_kind": "metadata-planner",
            "route_family": "metadata-family",
            "route_kind": "metadata-kind",
            "execution_path": "metadata-path",
            "local_execution_runtime_miss_reason": "metadata-reason",
        }));

        let usage = sanitize_usage_capture_controls_for_persistence(input);

        assert!(usage.candidate_id.is_none());
        assert!(usage.candidate_index.is_none());
        assert!(usage.key_name.is_none());
        assert!(usage.planner_kind.is_none());
        assert!(usage.route_family.is_none());
        assert!(usage.route_kind.is_none());
        assert!(usage.execution_path.is_none());
        assert!(usage.local_execution_runtime_miss_reason.is_none());
    }

    #[test]
    fn lifecycle_order_rejects_stale_and_equal_conflicting_terminal_events() {
        assert!(!usage_lifecycle_update_allowed(
            "completed",
            "pending",
            20,
            Some(20),
            "failed",
            "void",
            19,
            Some(19),
        ));
        assert!(!usage_lifecycle_update_allowed(
            "completed",
            "pending",
            20,
            Some(20),
            "failed",
            "void",
            20,
            Some(20),
        ));
        assert!(!usage_lifecycle_update_allowed(
            "completed",
            "pending",
            20,
            Some(20),
            "completed",
            "settled",
            20,
            Some(20),
        ));
        assert!(usage_lifecycle_update_allowed(
            "completed",
            "pending",
            20,
            Some(20),
            "failed",
            "void",
            21,
            Some(21),
        ));
    }

    #[test]
    fn lifecycle_order_allows_same_second_progress_and_fresh_void_recovery() {
        assert!(usage_lifecycle_update_allowed(
            "pending",
            "pending",
            20,
            None,
            "streaming",
            "pending",
            20,
            None,
        ));
        assert!(usage_lifecycle_update_allowed(
            "streaming",
            "pending",
            20,
            None,
            "completed",
            "pending",
            20,
            Some(20),
        ));
        assert!(usage_lifecycle_update_allowed(
            "failed",
            "void",
            20,
            Some(20),
            "completed",
            "pending",
            20,
            Some(20),
        ));
        assert!(!usage_lifecycle_update_allowed(
            "failed",
            "void",
            20,
            Some(21),
            "completed",
            "pending",
            20,
            Some(20),
        ));
    }

    use super::{api_key_usage_contribution, provider_api_key_usage_contribution};
    use crate::repository::usage::StoredRequestUsageAudit;

    fn authoritative_unpriced_usage() -> StoredRequestUsageAudit {
        let mut usage = StoredRequestUsageAudit::new(
            "usage-realtime".to_string(),
            "request-realtime".to_string(),
            None,
            Some("downstream-key".to_string()),
            None,
            None,
            "OpenAI".to_string(),
            "gpt-realtime".to_string(),
            None,
            Some("provider-realtime".to_string()),
            Some("endpoint-realtime".to_string()),
            Some("provider-key-realtime".to_string()),
            Some("realtime".to_string()),
            Some("openai:realtime".to_string()),
            Some("openai".to_string()),
            Some("realtime".to_string()),
            Some("openai:realtime".to_string()),
            Some("openai".to_string()),
            Some("realtime".to_string()),
            false,
            true,
            120,
            40,
            160,
            9.75,
            11.25,
            Some(200),
            None,
            None,
            Some(250),
            Some(30),
            "completed".to_string(),
            "void".to_string(),
            100,
            101,
            Some(102),
        )
        .expect("usage should build");
        usage.request_metadata = Some(json!({
            "usage_available": true,
            "usage_pricing_available": false,
            "realtime_session": {
                "input_audio_tokens": 20,
                "output_audio_tokens": 10,
            }
        }));
        usage
    }

    #[test]
    fn authoritative_unpriced_usage_contributes_tokens_but_never_cost() {
        let usage = authoritative_unpriced_usage();

        let provider = provider_api_key_usage_contribution(&usage)
            .expect("provider key contribution should exist");
        assert_eq!(provider.request_count, 1);
        assert_eq!(provider.success_count, 1);
        assert_eq!(provider.total_tokens, 160);
        assert_eq!(provider.total_cost_usd, 0.0);

        let downstream =
            api_key_usage_contribution(&usage).expect("API key contribution should exist");
        assert_eq!(downstream.total_requests, 1);
        assert_eq!(downstream.total_tokens, 160);
        assert_eq!(downstream.total_cost_usd, 0.0);
    }
}
