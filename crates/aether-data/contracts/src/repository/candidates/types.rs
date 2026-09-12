use std::collections::BTreeMap;

use async_trait::async_trait;

use crate::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};

const UNCLASSIFIED_CANDIDATE_ERROR_TYPE: &str = "unclassified_error";
const UNCLASSIFIED_CANDIDATE_SKIP_REASON: &str = "unclassified_skip";

macro_rules! define_candidate_diagnostic_categories {
    ($constant:ident, $predicate:ident, [$($value:literal),+ $(,)?]) => {
        pub const $constant: &[&str] = &[$($value),+];

        fn $predicate(value: &str) -> bool {
            matches!(value, $($value)|+)
        }
    };
}

define_candidate_diagnostic_categories!(
    REQUEST_CANDIDATE_SKIP_REASONS,
    is_known_request_candidate_skip_reason,
    [
        "account_quota_exhausted",
        "api_key_concurrency_limit_reached",
        "auth_api_key_concurrency_limit_reached",
        "auth_channel_mismatch",
        "auth_snapshot_missing",
        "endpoint_api_format_changed",
        "endpoint_inactive",
        "format_conversion_disabled",
        "gemini_file_mapping_mismatch",
        "key_api_format_disabled",
        "key_circuit_open",
        "key_health_score_zero",
        "key_inactive",
        "key_model_disabled",
        "key_model_not_allowed",
        "key_rpm_exhausted",
        "mapped_model_missing",
        "oauth_invalid",
        "pool_account_blocked",
        "pool_account_exhausted",
        "pool_active_probe_sealed",
        "pool_cooldown",
        "pool_cost_limit_reached",
        "pool_group_exhausted",
        "pool_key_lease_busy",
        "pool_score_member_missing",
        "provider_concurrency_limit_reached",
        "provider_inactive",
        "provider_key_concurrency_limit_reached",
        "provider_quota_blocked",
        "provider_request_body_build_failed",
        "provider_request_body_missing",
        "routing_profile_disallowed_key",
        "routing_profile_disallowed_provider",
        "transport_api_format_mismatch",
        "transport_api_format_unsupported",
        "transport_auth_unavailable",
        "transport_body_rules_apply_failed",
        "transport_body_rules_unsupported",
        "transport_body_rules_unsupported_for_binary_upload",
        "transport_custom_path_unsupported",
        "transport_endpoint_kind_unsupported",
        "transport_header_rules_apply_failed",
        "transport_header_rules_unsupported",
        "transport_oauth_resolution_unsupported",
        "transport_operation_unsupported",
        "transport_profile_unsupported",
        "transport_provider_type_unsupported",
        "transport_proxy_or_profile_unsupported",
        "transport_proxy_unsupported",
        "transport_snapshot_missing",
        "transport_unsupported",
        "upstream_url_missing",
    ]
);

pub const REQUEST_CANDIDATE_ERROR_TYPE_ALIASES: &[(&str, &str)] = &[
    ("connecttimeout", "connect_timeout"),
    ("firstbytetimeout", "first_byte_timeout"),
    ("protocolerror", "protocol_error"),
    ("proxyerror", "proxy_error"),
    ("readtimeout", "read_timeout"),
    ("tlserror", "tls_error"),
];

define_candidate_diagnostic_categories!(
    REQUEST_CANDIDATE_ERROR_TYPES,
    is_known_request_candidate_error_type,
    [
        "api_error",
        "authentication_error",
        "all_candidates_skipped",
        "cancelled",
        "candidate_list_empty",
        "chatgpt_web_image_execution_unavailable",
        "client_delivery_failed",
        "connect_timeout",
        "control_fallback",
        "downstream_disconnect",
        "execution_runtime_http_error",
        "execution_runtime_stream_chunk_decode_error",
        "execution_runtime_stream_frame_decode_error",
        "execution_runtime_stream_non_success_status",
        "execution_runtime_stream_read_error",
        "execution_runtime_stream_rewrite_error",
        "execution_runtime_stream_rewrite_flush_error",
        "execution_runtime_sync_json_stream_bridge_error",
        "execution_runtime_unavailable",
        "first_byte_timeout",
        "gateway_admission_failed",
        "gateway_admission_timeout",
        "grok_execution_unavailable",
        "grok_upstream_error",
        "image_sync_total_timeout",
        "internal",
        "invalid_provider_success_response",
        "invalid_request_error",
        "kiro_web_search_mcp_unavailable",
        "local_stream_candidate_watchdog_timeout",
        "local_stream_attempt_cancelled",
        "local_sync_attempt_aborted",
        "local_sync_attempt_cancelled",
        "not_found_error",
        "no_local_stream_plans",
        "no_local_sync_plans",
        "overloaded_error",
        "permission_error",
        "plan_usage_limit_exceeded",
        "provider_request_body_build_failed",
        "provider_request_body_missing",
        "protocol_error",
        "proxy_error",
        "rate_limit_error",
        "read_timeout",
        "resource_exhausted",
        "retryable_upstream_status",
        "server_error",
        "stream_http_error",
        "stream_missing_terminal_event",
        "stream_terminal_error",
        "success_failover_pattern",
        "tls_error",
        "upstream4xx",
        "upstream5xx",
        "upstream_error",
        "upstream_response_decode_failed",
        "upstream_response_too_large",
        "upstream_url_missing",
        "websocket_cancelled",
        "windsurf_native_execution_unavailable",
    ]
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestCandidateStatus {
    Available,
    Unused,
    Pending,
    Streaming,
    Success,
    Failed,
    Cancelled,
    Skipped,
}

impl RequestCandidateStatus {
    pub fn from_database(value: &str) -> Result<Self, crate::DataLayerError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "available" => Ok(Self::Available),
            "unused" => Ok(Self::Unused),
            "pending" => Ok(Self::Pending),
            "streaming" => Ok(Self::Streaming),
            "success" => Ok(Self::Success),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            "skipped" => Ok(Self::Skipped),
            other => Err(crate::DataLayerError::UnexpectedValue(format!(
                "unsupported request_candidates.status: {other}"
            ))),
        }
    }

    pub fn is_attempted(self, started_at_unix_ms: Option<u64>) -> bool {
        match self {
            Self::Available | Self::Unused | Self::Skipped => false,
            Self::Pending => started_at_unix_ms.is_some(),
            Self::Streaming | Self::Success | Self::Failed | Self::Cancelled => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredRequestCandidate {
    pub id: String,
    pub request_id: String,
    pub user_id: Option<String>,
    pub api_key_id: Option<String>,
    pub username: Option<String>,
    pub api_key_name: Option<String>,
    pub candidate_index: u32,
    pub retry_index: u32,
    pub provider_id: Option<String>,
    pub endpoint_id: Option<String>,
    pub key_id: Option<String>,
    pub status: RequestCandidateStatus,
    pub skip_reason: Option<String>,
    pub is_cached: bool,
    pub status_code: Option<u16>,
    pub error_type: Option<String>,
    pub error_message: Option<String>,
    pub latency_ms: Option<u64>,
    pub concurrent_requests: Option<u32>,
    pub extra_data: Option<serde_json::Value>,
    pub required_capabilities: Option<serde_json::Value>,
    pub created_at_unix_ms: u64,
    pub started_at_unix_ms: Option<u64>,
    pub finished_at_unix_ms: Option<u64>,
}

impl StoredRequestCandidate {
    /// Scheduling needs identity, status, counters and times, without diagnostic payloads.
    pub fn runtime_snapshot(&self) -> Self {
        Self {
            id: self.id.clone(),
            request_id: self.request_id.clone(),
            user_id: self.user_id.clone(),
            api_key_id: self.api_key_id.clone(),
            username: None,
            api_key_name: None,
            candidate_index: self.candidate_index,
            retry_index: self.retry_index,
            provider_id: self.provider_id.clone(),
            endpoint_id: self.endpoint_id.clone(),
            key_id: self.key_id.clone(),
            status: self.status,
            skip_reason: None,
            is_cached: self.is_cached,
            status_code: self.status_code,
            error_type: None,
            error_message: None,
            latency_ms: self.latency_ms,
            concurrent_requests: self.concurrent_requests,
            extra_data: None,
            required_capabilities: None,
            created_at_unix_ms: self.created_at_unix_ms,
            started_at_unix_ms: self.started_at_unix_ms,
            finished_at_unix_ms: self.finished_at_unix_ms,
        }
    }

    pub fn sanitize_for_persistence(&mut self) {
        self.username = None;
        self.api_key_name = None;
        self.skip_reason = sanitize_request_candidate_skip_reason(self.skip_reason.take());
        self.error_type = sanitize_request_candidate_error_type(self.error_type.take());
        self.error_message = self
            .error_message
            .take()
            .map(limit_candidate_diagnostic_text);
        self.extra_data =
            sanitize_request_candidate_extra_data_for_persistence(self.extra_data.take());
        self.required_capabilities =
            sanitize_request_candidate_required_capabilities(self.required_capabilities.take());
    }

    pub fn sanitize_sensitive_diagnostics(&mut self) {
        self.username = None;
        self.api_key_name = None;
        self.skip_reason = sanitize_request_candidate_skip_reason(self.skip_reason.take());
        self.error_type = sanitize_request_candidate_error_type(self.error_type.take());
        self.error_message = None;
        self.extra_data = sanitize_request_candidate_extra_data(self.extra_data.take());
        self.required_capabilities =
            sanitize_request_candidate_required_capabilities(self.required_capabilities.take());
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        request_id: String,
        user_id: Option<String>,
        api_key_id: Option<String>,
        username: Option<String>,
        api_key_name: Option<String>,
        candidate_index: i32,
        retry_index: i32,
        provider_id: Option<String>,
        endpoint_id: Option<String>,
        key_id: Option<String>,
        status: RequestCandidateStatus,
        skip_reason: Option<String>,
        is_cached: bool,
        status_code: Option<i32>,
        error_type: Option<String>,
        _error_message: Option<String>,
        latency_ms: Option<i32>,
        concurrent_requests: Option<i32>,
        extra_data: Option<serde_json::Value>,
        required_capabilities: Option<serde_json::Value>,
        created_at_unix_ms: i64,
        started_at_unix_ms: Option<i64>,
        finished_at_unix_ms: Option<i64>,
    ) -> Result<Self, crate::DataLayerError> {
        let candidate_index = u32::try_from(candidate_index).map_err(|_| {
            crate::DataLayerError::UnexpectedValue(format!(
                "invalid request_candidates.candidate_index: {candidate_index}"
            ))
        })?;
        let retry_index = u32::try_from(retry_index).map_err(|_| {
            crate::DataLayerError::UnexpectedValue(format!(
                "invalid request_candidates.retry_index: {retry_index}"
            ))
        })?;
        let status_code = status_code
            .map(|value| {
                u16::try_from(value).map_err(|_| {
                    crate::DataLayerError::UnexpectedValue(format!(
                        "invalid request_candidates.status_code: {value}"
                    ))
                })
            })
            .transpose()?;
        let latency_ms = latency_ms
            .map(|value| {
                u64::try_from(value).map_err(|_| {
                    crate::DataLayerError::UnexpectedValue(format!(
                        "invalid request_candidates.latency_ms: {value}"
                    ))
                })
            })
            .transpose()?;
        let concurrent_requests = concurrent_requests
            .map(|value| {
                u32::try_from(value).map_err(|_| {
                    crate::DataLayerError::UnexpectedValue(format!(
                        "invalid request_candidates.concurrent_requests: {value}"
                    ))
                })
            })
            .transpose()?;
        let created_at_unix_ms = u64::try_from(created_at_unix_ms).map_err(|_| {
            crate::DataLayerError::UnexpectedValue(format!(
                "invalid request_candidates.created_at_unix_ms: {created_at_unix_ms}"
            ))
        })?;
        let started_at_unix_ms = started_at_unix_ms
            .map(|value| {
                u64::try_from(value).map_err(|_| {
                    crate::DataLayerError::UnexpectedValue(format!(
                        "invalid request_candidates.started_at_unix_ms: {value}"
                    ))
                })
            })
            .transpose()?;
        let finished_at_unix_ms = finished_at_unix_ms
            .map(|value| {
                u64::try_from(value).map_err(|_| {
                    crate::DataLayerError::UnexpectedValue(format!(
                        "invalid request_candidates.finished_at_unix_ms: {value}"
                    ))
                })
            })
            .transpose()?;

        let mut candidate = Self {
            id,
            request_id,
            user_id,
            api_key_id,
            username,
            api_key_name,
            candidate_index,
            retry_index,
            provider_id,
            endpoint_id,
            key_id,
            status,
            skip_reason,
            is_cached,
            status_code,
            error_type,
            error_message: _error_message,
            latency_ms,
            concurrent_requests,
            extra_data,
            required_capabilities,
            created_at_unix_ms,
            started_at_unix_ms,
            finished_at_unix_ms,
        };
        candidate.sanitize_for_persistence();
        Ok(candidate)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestCandidateFinalStatus {
    Success,
    Failed,
    Cancelled,
    Streaming,
    Pending,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RequestCandidateTrace {
    pub request_id: String,
    pub total_candidates: usize,
    pub final_status: RequestCandidateFinalStatus,
    pub total_latency_ms: u64,
    pub candidates: Vec<StoredRequestCandidate>,
}

impl RequestCandidateTrace {
    pub fn sanitize_sensitive_diagnostics(&mut self) {
        for candidate in &mut self.candidates {
            candidate.sanitize_sensitive_diagnostics();
        }
    }

    pub fn from_candidates(
        request_id: impl Into<String>,
        mut all_candidates: Vec<StoredRequestCandidate>,
        attempted_only: bool,
    ) -> Option<Self> {
        for candidate in &mut all_candidates {
            candidate.sanitize_for_persistence();
        }
        if all_candidates.is_empty() {
            return None;
        }

        let candidates = if attempted_only {
            all_candidates
                .iter()
                .filter(|candidate| candidate.status.is_attempted(candidate.started_at_unix_ms))
                .cloned()
                .collect::<Vec<_>>()
        } else {
            all_candidates.clone()
        };

        let total_latency_ms = candidates
            .iter()
            .filter(|candidate| {
                matches!(
                    candidate.status,
                    RequestCandidateStatus::Success
                        | RequestCandidateStatus::Failed
                        | RequestCandidateStatus::Cancelled
                )
            })
            .map(|candidate| {
                candidate.latency_ms.unwrap_or_else(|| {
                    candidate
                        .finished_at_unix_ms
                        .zip(candidate.started_at_unix_ms)
                        .map(|(finished_at, started_at)| finished_at.saturating_sub(started_at))
                        .unwrap_or(0)
                })
            })
            .fold(0_u64, u64::saturating_add);
        let final_status_source = if attempted_only && candidates.is_empty() {
            &all_candidates
        } else {
            &candidates
        };

        Some(Self {
            request_id: request_id.into(),
            total_candidates: candidates.len(),
            final_status: derive_request_candidate_final_status(final_status_source),
            total_latency_ms,
            candidates,
        })
    }
}

pub fn derive_request_candidate_final_status(
    candidates: &[StoredRequestCandidate],
) -> RequestCandidateFinalStatus {
    let has_success = candidates
        .iter()
        .any(|candidate| candidate.status == RequestCandidateStatus::Success);
    if has_success {
        return RequestCandidateFinalStatus::Success;
    }

    let has_failed = candidates
        .iter()
        .any(|candidate| candidate.status == RequestCandidateStatus::Failed);
    if has_failed {
        return RequestCandidateFinalStatus::Failed;
    }

    let has_cancelled = candidates
        .iter()
        .any(|candidate| candidate.status == RequestCandidateStatus::Cancelled);
    if has_cancelled {
        return RequestCandidateFinalStatus::Cancelled;
    }

    if candidates
        .iter()
        .any(|candidate| candidate.status == RequestCandidateStatus::Streaming)
    {
        return RequestCandidateFinalStatus::Streaming;
    }

    if candidates
        .iter()
        .any(|candidate| candidate.status == RequestCandidateStatus::Pending)
    {
        return RequestCandidateFinalStatus::Pending;
    }

    let has_legacy_success_status_code = candidates
        .iter()
        .any(|candidate| matches!(candidate.status_code, Some(status_code) if (200..300).contains(&status_code)));
    if has_legacy_success_status_code {
        return RequestCandidateFinalStatus::Success;
    }

    RequestCandidateFinalStatus::Failed
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DecisionTraceCandidate {
    #[serde(flatten)]
    pub candidate: StoredRequestCandidate,
    pub provider_name: Option<String>,
    pub provider_website: Option<String>,
    pub provider_type: Option<String>,
    pub provider_priority: Option<i32>,
    pub provider_keep_priority_on_conversion: Option<bool>,
    pub provider_enable_format_conversion: Option<bool>,
    pub endpoint_api_format: Option<String>,
    pub endpoint_api_family: Option<String>,
    pub endpoint_kind: Option<String>,
    pub endpoint_format_acceptance_config: Option<serde_json::Value>,
    pub provider_key_name: Option<String>,
    pub provider_key_auth_type: Option<String>,
    pub provider_key_api_formats: Option<serde_json::Value>,
    pub provider_key_internal_priority: Option<i32>,
    pub provider_key_global_priority_by_format: Option<serde_json::Value>,
    pub provider_key_capabilities: Option<serde_json::Value>,
    pub provider_key_is_active: Option<bool>,
}

impl DecisionTraceCandidate {
    pub fn sanitize_for_admin(&mut self) {
        self.candidate.sanitize_for_persistence();
        self.sanitize_catalog_metadata();
    }

    pub fn sanitize_sensitive_diagnostics(&mut self) {
        self.candidate.sanitize_sensitive_diagnostics();
        self.sanitize_catalog_metadata();
    }

    fn sanitize_catalog_metadata(&mut self) {
        self.provider_website = self
            .provider_website
            .take()
            .and_then(|value| sanitize_candidate_url(&value));
        self.endpoint_format_acceptance_config = None;
        self.provider_key_api_formats =
            sanitize_request_candidate_api_formats(self.provider_key_api_formats.take());
        self.provider_key_global_priority_by_format = None;
        self.provider_key_capabilities =
            sanitize_request_candidate_required_capabilities(self.provider_key_capabilities.take());
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DecisionTrace {
    pub request_id: String,
    pub total_candidates: usize,
    pub final_status: RequestCandidateFinalStatus,
    pub total_latency_ms: u64,
    pub candidates: Vec<DecisionTraceCandidate>,
}

impl DecisionTrace {
    pub fn sanitize_sensitive_diagnostics(&mut self) {
        for item in &mut self.candidates {
            item.sanitize_sensitive_diagnostics();
        }
    }
}

pub fn build_decision_trace(
    trace: RequestCandidateTrace,
    providers: Vec<StoredProviderCatalogProvider>,
    endpoints: Vec<StoredProviderCatalogEndpoint>,
    keys: Vec<StoredProviderCatalogKey>,
) -> DecisionTrace {
    let provider_map = providers
        .into_iter()
        .map(|item| (item.id.clone(), item))
        .collect::<BTreeMap<_, _>>();
    let endpoint_map = endpoints
        .into_iter()
        .map(|item| (item.id.clone(), item))
        .collect::<BTreeMap<_, _>>();
    let key_map = keys
        .into_iter()
        .map(|item| (item.id.clone(), item))
        .collect::<BTreeMap<_, _>>();

    let mut trace = DecisionTrace {
        request_id: trace.request_id,
        total_candidates: trace.total_candidates,
        final_status: trace.final_status,
        total_latency_ms: trace.total_latency_ms,
        candidates: trace
            .candidates
            .into_iter()
            .map(|candidate| {
                enrich_decision_trace_candidate(candidate, &provider_map, &endpoint_map, &key_map)
            })
            .collect(),
    };
    for item in &mut trace.candidates {
        item.sanitize_for_admin();
    }
    trace
}

fn enrich_decision_trace_candidate(
    candidate: StoredRequestCandidate,
    provider_map: &BTreeMap<String, StoredProviderCatalogProvider>,
    endpoint_map: &BTreeMap<String, StoredProviderCatalogEndpoint>,
    key_map: &BTreeMap<String, StoredProviderCatalogKey>,
) -> DecisionTraceCandidate {
    let provider = candidate
        .provider_id
        .as_ref()
        .and_then(|provider_id| provider_map.get(provider_id));
    let endpoint = candidate
        .endpoint_id
        .as_ref()
        .and_then(|endpoint_id| endpoint_map.get(endpoint_id));
    let provider_key = candidate
        .key_id
        .as_ref()
        .and_then(|key_id| key_map.get(key_id));

    DecisionTraceCandidate {
        provider_name: provider.map(|item| item.name.clone()),
        provider_website: provider.and_then(|item| item.website.clone()),
        provider_type: provider.map(|item| item.provider_type.clone()),
        provider_priority: provider.map(|item| item.provider_priority),
        provider_keep_priority_on_conversion: provider.map(|item| item.keep_priority_on_conversion),
        provider_enable_format_conversion: provider.map(|item| item.enable_format_conversion),
        endpoint_api_format: endpoint.map(|item| item.api_format.clone()),
        endpoint_api_family: endpoint.and_then(|item| item.api_family.clone()),
        endpoint_kind: endpoint.and_then(|item| item.endpoint_kind.clone()),
        endpoint_format_acceptance_config: endpoint
            .and_then(|item| item.format_acceptance_config.clone()),
        provider_key_name: provider_key
            .map(|item| item.name.clone())
            .or_else(|| candidate.api_key_name.clone()),
        provider_key_auth_type: provider_key.map(|item| item.auth_type.clone()),
        provider_key_api_formats: provider_key.and_then(|item| item.api_formats.clone()),
        provider_key_internal_priority: provider_key.map(|item| item.internal_priority),
        provider_key_global_priority_by_format: provider_key
            .and_then(|item| item.global_priority_by_format.clone()),
        provider_key_capabilities: provider_key.and_then(|item| item.capabilities.clone()),
        provider_key_is_active: provider_key.map(|item| item.is_active),
        candidate,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PublicHealthStatusCount {
    pub endpoint_id: String,
    pub status: RequestCandidateStatus,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PublicHealthTimelineBucket {
    pub endpoint_id: String,
    pub segment_idx: u32,
    pub total_count: u64,
    pub success_count: u64,
    pub failed_count: u64,
    pub min_created_at_unix_ms: Option<u64>,
    pub max_created_at_unix_ms: Option<u64>,
}

#[async_trait]
pub trait RequestCandidateReadRepository: Send + Sync {
    async fn list_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Vec<StoredRequestCandidate>, crate::DataLayerError>;

    async fn list_attempted_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Vec<StoredRequestCandidate>, crate::DataLayerError> {
        Ok(self
            .list_by_request_id(request_id)
            .await?
            .into_iter()
            .filter(|candidate| candidate.status.is_attempted(candidate.started_at_unix_ms))
            .collect())
    }

    async fn list_recent(
        &self,
        limit: usize,
    ) -> Result<Vec<StoredRequestCandidate>, crate::DataLayerError>;

    /// Same ordering and limit as `list_recent`, omitting diagnostic fields.
    async fn list_recent_runtime(
        &self,
        limit: usize,
    ) -> Result<Vec<StoredRequestCandidate>, crate::DataLayerError> {
        Ok(self
            .list_recent(limit)
            .await?
            .iter()
            .map(StoredRequestCandidate::runtime_snapshot)
            .collect())
    }

    async fn list_by_provider_id(
        &self,
        provider_id: &str,
        limit: usize,
    ) -> Result<Vec<StoredRequestCandidate>, crate::DataLayerError>;

    async fn list_finalized_by_endpoint_ids_since(
        &self,
        endpoint_ids: &[String],
        since_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredRequestCandidate>, crate::DataLayerError>;

    async fn count_finalized_statuses_by_endpoint_ids_since(
        &self,
        endpoint_ids: &[String],
        since_unix_secs: u64,
    ) -> Result<Vec<PublicHealthStatusCount>, crate::DataLayerError>;

    async fn aggregate_finalized_timeline_by_endpoint_ids_since(
        &self,
        endpoint_ids: &[String],
        since_unix_secs: u64,
        until_unix_secs: u64,
        segments: u32,
    ) -> Result<Vec<PublicHealthTimelineBucket>, crate::DataLayerError>;
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UpsertRequestCandidateRecord {
    pub id: String,
    pub request_id: String,
    pub user_id: Option<String>,
    pub api_key_id: Option<String>,
    pub username: Option<String>,
    pub api_key_name: Option<String>,
    pub candidate_index: u32,
    pub retry_index: u32,
    pub provider_id: Option<String>,
    pub endpoint_id: Option<String>,
    pub key_id: Option<String>,
    pub status: RequestCandidateStatus,
    pub skip_reason: Option<String>,
    pub is_cached: Option<bool>,
    pub status_code: Option<u16>,
    pub error_type: Option<String>,
    pub error_message: Option<String>,
    pub latency_ms: Option<u64>,
    pub concurrent_requests: Option<u32>,
    pub extra_data: Option<serde_json::Value>,
    pub required_capabilities: Option<serde_json::Value>,
    pub created_at_unix_ms: Option<u64>,
    pub started_at_unix_ms: Option<u64>,
    pub finished_at_unix_ms: Option<u64>,
}

impl UpsertRequestCandidateRecord {
    pub fn sanitize_for_persistence(&mut self) {
        self.username = None;
        self.api_key_name = None;
        self.skip_reason = sanitize_request_candidate_skip_reason(self.skip_reason.take());
        self.error_type = sanitize_request_candidate_error_type(self.error_type.take());
        self.error_message = self
            .error_message
            .take()
            .map(limit_candidate_diagnostic_text);
        self.extra_data =
            sanitize_request_candidate_extra_data_for_persistence(self.extra_data.take());
        self.required_capabilities =
            sanitize_request_candidate_required_capabilities(self.required_capabilities.take());
    }

    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        if self.id.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "request candidate upsert id cannot be empty".to_string(),
            ));
        }
        if self.request_id.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "request candidate upsert request_id cannot be empty".to_string(),
            ));
        }
        for (field, value) in [
            ("id", Some(self.id.as_str())),
            ("request_id", Some(self.request_id.as_str())),
            ("user_id", self.user_id.as_deref()),
            ("api_key_id", self.api_key_id.as_deref()),
            ("provider_id", self.provider_id.as_deref()),
            ("endpoint_id", self.endpoint_id.as_deref()),
            ("key_id", self.key_id.as_deref()),
        ] {
            if value.is_some_and(|value| value.contains('\0')) {
                return Err(crate::DataLayerError::InvalidInput(format!(
                    "request candidate upsert {field} cannot contain NUL"
                )));
            }
        }
        Ok(())
    }
}

pub fn sanitize_request_candidate_skip_reason(value: Option<String>) -> Option<String> {
    let value = value?;
    let normalized = value.trim().to_ascii_lowercase();
    let safe = if is_known_request_candidate_skip_reason(normalized.as_str()) {
        normalized
    } else {
        UNCLASSIFIED_CANDIDATE_SKIP_REASON.to_string()
    };
    Some(safe)
}

pub fn sanitize_request_candidate_error_type(value: Option<String>) -> Option<String> {
    let value = value?;
    let normalized = value.trim().to_ascii_lowercase();
    let safe = REQUEST_CANDIDATE_ERROR_TYPE_ALIASES
        .iter()
        .find_map(|(alias, canonical)| (normalized == *alias).then_some(*canonical))
        .map(str::to_string)
        .or_else(|| {
            is_known_request_candidate_error_type(normalized.as_str()).then_some(normalized)
        })
        .unwrap_or_else(|| UNCLASSIFIED_CANDIDATE_ERROR_TYPE.to_string());
    Some(safe)
}

const MAX_CANDIDATE_DIAGNOSTIC_BYTES: usize = 65_536;

fn limit_candidate_diagnostic_text(mut text: String) -> String {
    const SUFFIX: &str = "...[truncated]";
    if text.len() > MAX_CANDIDATE_DIAGNOSTIC_BYTES {
        let mut boundary = MAX_CANDIDATE_DIAGNOSTIC_BYTES - SUFFIX.len();
        while !text.is_char_boundary(boundary) {
            boundary -= 1;
        }
        text.truncate(boundary);
        text.push_str(SUFFIX);
    }
    text
}

fn limit_candidate_diagnostic_value(value: &serde_json::Value) -> serde_json::Value {
    if let Some(text) = value.as_str() {
        return serde_json::Value::String(limit_candidate_diagnostic_text(text.to_string()));
    }
    let serialized = value.to_string();
    if serialized.len() > MAX_CANDIDATE_DIAGNOSTIC_BYTES {
        serde_json::Value::String(limit_candidate_diagnostic_text(serialized))
    } else {
        value.clone()
    }
}

pub fn sanitize_request_candidate_extra_data_for_persistence(
    extra_data: Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    let object = extra_data.as_ref()?.as_object()?;
    let mut sanitized = sanitize_candidate_extra_data_object(object);
    for (key, fields) in [
        ("upstream_response", &["headers", "body"][..]),
        ("error_flow", &["message"][..]),
        (
            "failure_diagnostic",
            &[
                "path",
                "field_path",
                "message",
                "type",
                "reason",
                "details",
                "stage",
                "source_format",
                "target_format",
                "safe_to_show",
            ][..],
        ),
        (
            "request_conversion_error",
            &["path", "field_path", "message", "type", "reason"][..],
        ),
        (
            "request_body_build_error",
            &["path", "field_path", "message", "type", "reason"][..],
        ),
    ] {
        let Some(diagnostic) = object.get(key).and_then(serde_json::Value::as_object) else {
            continue;
        };
        let mut summary = sanitized
            .remove(key)
            .and_then(|value| match value {
                serde_json::Value::Object(object) => Some(object),
                _ => None,
            })
            .unwrap_or_default();
        for field in fields {
            if let Some(value) = diagnostic.get(*field).filter(|value| !value.is_null()) {
                summary.insert(
                    (*field).to_string(),
                    limit_candidate_diagnostic_value(value),
                );
            }
        }
        if !summary.is_empty() {
            sanitized.insert(key.to_string(), serde_json::Value::Object(summary));
        }
    }
    (!sanitized.is_empty()).then_some(serde_json::Value::Object(sanitized))
}

pub fn sanitize_request_candidate_extra_data(
    extra_data: Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    let serde_json::Value::Object(object) = extra_data? else {
        return None;
    };
    let sanitized = sanitize_candidate_extra_data_object(&object);
    (!sanitized.is_empty()).then_some(serde_json::Value::Object(sanitized))
}

fn sanitize_candidate_extra_data_object(
    object: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    let mut sanitized = serde_json::Map::new();

    for field in ["gateway_execution_runtime", "stream_completed", "cache_1h"] {
        insert_candidate_bool(object, &mut sanitized, field);
    }
    for field in ["first_byte_time_ms", "pool_key_index"] {
        insert_candidate_u64(object, &mut sanitized, field);
    }
    insert_candidate_i64(object, &mut sanitized, "priority_slot");
    insert_candidate_u64(object, &mut sanitized, "ranking_index");

    insert_candidate_known_string(object, &mut sanitized, "phase", sanitize_candidate_phase);
    for field in [
        "client_api_format",
        "provider_api_format",
        "client_contract",
        "provider_contract",
    ] {
        insert_candidate_known_string(object, &mut sanitized, field, sanitize_candidate_api_format);
    }
    insert_candidate_known_string(
        object,
        &mut sanitized,
        "execution_strategy",
        sanitize_candidate_execution_strategy,
    );
    insert_candidate_known_string(
        object,
        &mut sanitized,
        "conversion_mode",
        sanitize_candidate_conversion_mode,
    );
    insert_candidate_known_string(
        object,
        &mut sanitized,
        "ranking_mode",
        sanitize_candidate_ranking_mode,
    );
    insert_candidate_known_string(
        object,
        &mut sanitized,
        "priority_mode",
        sanitize_candidate_priority_mode,
    );
    insert_candidate_known_string(
        object,
        &mut sanitized,
        "promoted_by",
        sanitize_candidate_promotion_reason,
    );
    insert_candidate_known_string(
        object,
        &mut sanitized,
        "demoted_by",
        sanitize_candidate_demotion_reason,
    );
    insert_candidate_known_string(object, &mut sanitized, "source", sanitize_candidate_source);
    insert_candidate_known_string(
        object,
        &mut sanitized,
        "execution_path",
        sanitize_candidate_execution_path,
    );

    if let Some(url) = object
        .get("upstream_url")
        .and_then(serde_json::Value::as_str)
        .and_then(sanitize_candidate_url)
    {
        sanitized.insert("upstream_url".to_string(), serde_json::Value::String(url));
    }
    if let Some(summary) = object
        .get("header_rules")
        .and_then(sanitize_candidate_rules_summary)
    {
        sanitized.insert("header_rules".to_string(), summary);
    }
    if let Some(summary) = object
        .get("body_rules")
        .and_then(sanitize_candidate_rules_summary)
    {
        sanitized.insert("body_rules".to_string(), summary);
    }
    if let Some(summary) = object.get("proxy").and_then(sanitize_candidate_proxy) {
        sanitized.insert("proxy".to_string(), summary);
    }
    if let Some(summary) = object
        .get("error_flow")
        .and_then(sanitize_candidate_error_flow)
    {
        sanitized.insert("error_flow".to_string(), summary);
    }
    if let Some(summary) = object
        .get("routing_trace")
        .and_then(sanitize_candidate_routing_trace)
    {
        sanitized.insert("routing_trace".to_string(), summary);
    }

    if let Some(summary) = object
        .get("upstream_response")
        .and_then(sanitize_candidate_upstream_response)
    {
        sanitized.insert("upstream_response".to_string(), summary);
    }
    if let Some(progress) = object
        .get("image_progress")
        .and_then(sanitize_candidate_image_progress)
    {
        sanitized.insert("image_progress".to_string(), progress);
    }
    if let Some(exhaustion) = object
        .get("pool_group_exhaustion")
        .and_then(sanitize_candidate_pool_group_exhaustion)
    {
        sanitized.insert("pool_group_exhaustion".to_string(), exhaustion);
    }

    sanitized
}

pub fn sanitize_request_candidate_required_capabilities(
    required_capabilities: Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    let serde_json::Value::Object(object) = required_capabilities? else {
        return None;
    };
    let mut sanitized = serde_json::Map::new();
    for capability in [
        "cache_1h",
        "context_1m",
        "gemini_files",
        "streaming",
        "vision",
    ] {
        let Some(enabled) = object
            .get(capability)
            .or_else(|| {
                object
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case(capability))
                    .map(|(_, value)| value)
            })
            .and_then(sanitize_candidate_capability_value)
        else {
            continue;
        };
        sanitized.insert(capability.to_string(), serde_json::Value::Bool(enabled));
    }
    (!sanitized.is_empty()).then_some(serde_json::Value::Object(sanitized))
}

pub fn sanitize_request_candidate_api_formats(
    api_formats: Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    let serde_json::Value::Array(values) = api_formats? else {
        return None;
    };
    let mut sanitized = Vec::new();
    for format in values
        .iter()
        .filter_map(serde_json::Value::as_str)
        .filter_map(sanitize_candidate_api_format)
    {
        if sanitized
            .iter()
            .any(|existing: &serde_json::Value| existing.as_str() == Some(format))
        {
            continue;
        }
        sanitized.push(serde_json::Value::String(format.to_string()));
    }
    (!sanitized.is_empty()).then_some(serde_json::Value::Array(sanitized))
}

fn sanitize_candidate_capability_value(value: &serde_json::Value) -> Option<bool> {
    match value {
        serde_json::Value::Bool(value) => Some(*value),
        serde_json::Value::String(value) => match value.trim().to_ascii_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        serde_json::Value::Number(value) => value
            .as_i64()
            .map(|value| value > 0)
            .or_else(|| value.as_u64().map(|value| value > 0))
            .or_else(|| value.as_f64().map(|value| value > 0.0)),
        _ => None,
    }
}

fn sanitize_candidate_url(value: &str) -> Option<String> {
    let mut url = url::Url::parse(value.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }

    url.set_username("").ok()?;
    url.set_password(None).ok()?;
    url.set_path("/");
    url.set_query(None);
    url.set_fragment(None);
    Some(url.into())
}

fn sanitize_candidate_rules_summary(value: &serde_json::Value) -> Option<serde_json::Value> {
    if let Some(summary) = value.as_object() {
        let mut sanitized = serde_json::Map::new();
        for field in ["count", "enabled_count", "conditional_count"] {
            insert_candidate_u64(summary, &mut sanitized, field);
        }
        if let Some(action_counts) = summary
            .get("action_counts")
            .and_then(sanitize_candidate_rule_action_counts)
        {
            sanitized.insert("action_counts".to_string(), action_counts);
        }
        return (!sanitized.is_empty()).then_some(serde_json::Value::Object(sanitized));
    }

    let rules = value.as_array()?;
    let mut summary = serde_json::Map::new();
    summary.insert(
        "count".to_string(),
        serde_json::Value::Number((rules.len() as u64).into()),
    );

    let mut enabled_count = 0_u64;
    let mut conditional_count = 0_u64;
    let mut action_counts = serde_json::Map::new();
    for rule in rules.iter().filter_map(serde_json::Value::as_object) {
        if rule.get("enabled").and_then(serde_json::Value::as_bool) == Some(false) {
            continue;
        }
        enabled_count = enabled_count.saturating_add(1);
        if rule.get("condition").is_some_and(|value| !value.is_null()) {
            conditional_count = conditional_count.saturating_add(1);
        }
        let Some(action) = rule
            .get("action")
            .or_else(|| rule.get("op"))
            .and_then(serde_json::Value::as_str)
            .and_then(sanitize_candidate_rule_action)
        else {
            continue;
        };
        let count = action_counts
            .get(action)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default()
            .saturating_add(1);
        action_counts.insert(action.to_string(), serde_json::Value::Number(count.into()));
    }
    summary.insert(
        "enabled_count".to_string(),
        serde_json::Value::Number(enabled_count.into()),
    );
    summary.insert(
        "conditional_count".to_string(),
        serde_json::Value::Number(conditional_count.into()),
    );
    if !action_counts.is_empty() {
        summary.insert(
            "action_counts".to_string(),
            serde_json::Value::Object(action_counts),
        );
    }
    Some(serde_json::Value::Object(summary))
}

fn sanitize_candidate_rule_action_counts(value: &serde_json::Value) -> Option<serde_json::Value> {
    let counts = value.as_object()?;
    let mut sanitized = serde_json::Map::new();
    for action in [
        "add",
        "append",
        "drop",
        "insert",
        "regex_replace",
        "remove",
        "rename",
        "replace",
        "set",
    ] {
        insert_candidate_u64(counts, &mut sanitized, action);
    }
    (!sanitized.is_empty()).then_some(serde_json::Value::Object(sanitized))
}

fn sanitize_candidate_rule_action(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "add" => Some("add"),
        "append" => Some("append"),
        "drop" => Some("drop"),
        "insert" => Some("insert"),
        "regex_replace" => Some("regex_replace"),
        "remove" => Some("remove"),
        "rename" => Some("rename"),
        "replace" => Some("replace"),
        "set" => Some("set"),
        _ => None,
    }
}

fn sanitize_candidate_proxy(value: &serde_json::Value) -> Option<serde_json::Value> {
    let object = value.as_object()?;
    let mut summary = serde_json::Map::new();
    insert_candidate_known_string(object, &mut summary, "mode", sanitize_candidate_proxy_mode);
    insert_candidate_known_string(
        object,
        &mut summary,
        "source",
        sanitize_candidate_proxy_source,
    );
    if let Some(url) = object
        .get("url")
        .and_then(serde_json::Value::as_str)
        .and_then(sanitize_candidate_url)
    {
        summary.insert("url".to_string(), serde_json::Value::String(url));
    }
    for field in ["ttfb_ms", "connection_acquire_ms", "response_wait_ms"] {
        insert_candidate_u64(object, &mut summary, field);
    }
    if let Some(timing) = object
        .get("timing")
        .and_then(sanitize_candidate_proxy_timing)
    {
        summary.insert("timing".to_string(), timing);
    }
    (!summary.is_empty()).then_some(serde_json::Value::Object(summary))
}

fn sanitize_candidate_proxy_timing(value: &serde_json::Value) -> Option<serde_json::Value> {
    let object = value.as_object()?;
    let mut summary = serde_json::Map::new();
    for field in [
        "connection_acquire_ms",
        "connection_ms",
        "response_wait_ms",
        "ttfb_ms",
        "upstream_processing_ms",
    ] {
        insert_candidate_u64(object, &mut summary, field);
    }
    (!summary.is_empty()).then_some(serde_json::Value::Object(summary))
}

fn sanitize_candidate_proxy_mode(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "direct" => Some("direct"),
        "manual" => Some("manual"),
        "node" => Some("node"),
        "system" => Some("system"),
        "tunnel" => Some("tunnel"),
        _ => None,
    }
}

fn sanitize_candidate_proxy_source(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "endpoint" => Some("endpoint"),
        "key" => Some("key"),
        "provider" => Some("provider"),
        "system" => Some("system"),
        "tunnel_affinity" => Some("tunnel_affinity"),
        _ => None,
    }
}

fn sanitize_candidate_error_flow(value: &serde_json::Value) -> Option<serde_json::Value> {
    let object = value.as_object()?;
    let mut summary = serde_json::Map::new();
    insert_candidate_known_string(
        object,
        &mut summary,
        "stage",
        sanitize_candidate_error_stage,
    );
    insert_candidate_known_string(
        object,
        &mut summary,
        "source",
        sanitize_candidate_error_source,
    );
    insert_candidate_known_string(
        object,
        &mut summary,
        "classification",
        sanitize_candidate_error_classification,
    );
    insert_candidate_known_string(
        object,
        &mut summary,
        "decision",
        sanitize_candidate_error_decision,
    );
    insert_candidate_known_string(
        object,
        &mut summary,
        "propagation",
        sanitize_candidate_error_propagation,
    );
    for field in ["retryable", "safe_to_expose", "safe_to_expose_upstream"] {
        insert_candidate_bool(object, &mut summary, field);
    }
    if let Some(status_code) = object
        .get("status_code")
        .and_then(serde_json::Value::as_u64)
        .filter(|status_code| *status_code <= u64::from(u16::MAX))
    {
        summary.insert(
            "status_code".to_string(),
            serde_json::Value::Number(status_code.into()),
        );
    }
    (!summary.is_empty()).then_some(serde_json::Value::Object(summary))
}

fn sanitize_candidate_error_stage(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "candidate" => Some("candidate"),
        "client" => Some("client"),
        "gateway" => Some("gateway"),
        "request" => Some("request"),
        "upstream" => Some("upstream"),
        _ => None,
    }
}

fn sanitize_candidate_error_source(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "client" => Some("client"),
        "client_response" => Some("client_response"),
        "gateway" => Some("gateway"),
        "request" => Some("request"),
        "summary" => Some("summary"),
        "upstream" => Some("upstream"),
        "upstream_response" => Some("upstream_response"),
        _ => None,
    }
}

fn sanitize_candidate_error_classification(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "retry_status_code" => Some("retry_status_code"),
        "retry_success_pattern" => Some("retry_success_pattern"),
        "retry_transport_error" => Some("retry_transport_error"),
        "retry_upstream_failure" => Some("retry_upstream_failure"),
        "stop_cyber_policy" => Some("stop_cyber_policy"),
        "stop_error_pattern" => Some("stop_error_pattern"),
        "stop_execution_error" => Some("stop_execution_error"),
        "stop_status_code" => Some("stop_status_code"),
        "stop_transport_error" => Some("stop_transport_error"),
        "use_default" => Some("use_default"),
        _ => None,
    }
}

fn sanitize_candidate_error_decision(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "retry_next_candidate" => Some("retry_next_candidate"),
        "stop_local_failover" => Some("stop_local_failover"),
        "use_default" => Some("use_default"),
        _ => None,
    }
}

fn sanitize_candidate_error_propagation(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "captured" => Some("captured"),
        "converted" => Some("converted"),
        "local" => Some("local"),
        "none" => Some("none"),
        "passthrough" => Some("passthrough"),
        "suppressed" => Some("suppressed"),
        _ => None,
    }
}

fn sanitize_candidate_routing_trace(value: &serde_json::Value) -> Option<serde_json::Value> {
    let object = value.as_object()?;
    let mut summary = serde_json::Map::new();
    insert_candidate_i64(object, &mut summary, "group_version");
    insert_candidate_known_string(
        object,
        &mut summary,
        "selection_source",
        sanitize_candidate_routing_selection_source,
    );
    insert_candidate_known_string(
        object,
        &mut summary,
        "client_api_format",
        sanitize_candidate_api_format,
    );
    for (field, output) in [
        ("selected_rules", "selected_rule_count"),
        ("global_candidates", "global_candidate_count"),
        ("pool_expansion", "pool_expansion_count"),
    ] {
        if let Some(count) = object
            .get(field)
            .and_then(serde_json::Value::as_array)
            .map(Vec::len)
            .map(|value| value as u64)
            .or_else(|| object.get(output).and_then(serde_json::Value::as_u64))
        {
            summary.insert(output.to_string(), serde_json::Value::Number(count.into()));
        }
    }
    for field in [
        "client_request_patch_summary",
        "provider_request_patch_summary",
    ] {
        if let Some(patch) = object
            .get(field)
            .and_then(sanitize_candidate_routing_patch_summary)
        {
            summary.insert(field.to_string(), patch);
        }
    }
    if let Some(facts) = object
        .get("runtime_facts")
        .and_then(sanitize_candidate_routing_runtime_facts)
    {
        summary.insert("runtime_facts".to_string(), facts);
    }
    (!summary.is_empty()).then_some(serde_json::Value::Object(summary))
}

fn sanitize_candidate_routing_patch_summary(
    value: &serde_json::Value,
) -> Option<serde_json::Value> {
    let object = value.as_object()?;
    let mut summary = serde_json::Map::new();
    for (field, output) in [
        ("body_paths", "body_patch_count"),
        ("header_names", "header_patch_count"),
    ] {
        if let Some(count) = object
            .get(field)
            .and_then(serde_json::Value::as_array)
            .map(Vec::len)
            .map(|value| value as u64)
            .or_else(|| object.get(output).and_then(serde_json::Value::as_u64))
        {
            summary.insert(output.to_string(), serde_json::Value::Number(count.into()));
        }
    }
    (!summary.is_empty()).then_some(serde_json::Value::Object(summary))
}

fn sanitize_candidate_routing_runtime_facts(
    value: &serde_json::Value,
) -> Option<serde_json::Value> {
    let object = value.as_object()?;
    let mut summary = serde_json::Map::new();
    insert_candidate_bool(object, &mut summary, "cache_affinity_hit");
    insert_candidate_known_string(
        object,
        &mut summary,
        "scheduler_mode",
        sanitize_candidate_routing_scheduler_mode,
    );
    insert_candidate_known_string(
        object,
        &mut summary,
        "priority_mode",
        sanitize_candidate_routing_priority_mode,
    );
    (!summary.is_empty()).then_some(serde_json::Value::Object(summary))
}

fn sanitize_candidate_routing_selection_source(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "admin_dry_run" => Some("admin_dry_run"),
        "api_key_default" => Some("api_key_default"),
        "explicit" => Some("explicit"),
        "explicit_header" => Some("explicit_header"),
        "system_default" => Some("system_default"),
        "user_default" => Some("user_default"),
        "user_group_default" => Some("user_group_default"),
        _ => None,
    }
}

fn sanitize_candidate_routing_scheduler_mode(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "cache_affinity" | "cacheaffinity" => Some("cache_affinity"),
        "fixed_order" | "fixedorder" => Some("fixed_order"),
        "load_balance" | "loadbalance" => Some("load_balance"),
        _ => None,
    }
}

fn sanitize_candidate_routing_priority_mode(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "global_key" | "globalkey" => Some("global_key"),
        "provider" => Some("provider"),
        _ => None,
    }
}

fn insert_candidate_bool(
    source: &serde_json::Map<String, serde_json::Value>,
    target: &mut serde_json::Map<String, serde_json::Value>,
    field: &str,
) {
    if let Some(value) = source.get(field).and_then(serde_json::Value::as_bool) {
        target.insert(field.to_string(), serde_json::Value::Bool(value));
    }
}

fn insert_candidate_u64(
    source: &serde_json::Map<String, serde_json::Value>,
    target: &mut serde_json::Map<String, serde_json::Value>,
    field: &str,
) {
    if let Some(value) = source.get(field).and_then(serde_json::Value::as_u64) {
        target.insert(field.to_string(), serde_json::Value::Number(value.into()));
    }
}

fn insert_candidate_i64(
    source: &serde_json::Map<String, serde_json::Value>,
    target: &mut serde_json::Map<String, serde_json::Value>,
    field: &str,
) {
    if let Some(value) = source.get(field).and_then(serde_json::Value::as_i64) {
        target.insert(field.to_string(), serde_json::Value::Number(value.into()));
    }
}

fn insert_candidate_known_string(
    source: &serde_json::Map<String, serde_json::Value>,
    target: &mut serde_json::Map<String, serde_json::Value>,
    field: &str,
    sanitize: fn(&str) -> Option<&'static str>,
) {
    if let Some(value) = source
        .get(field)
        .and_then(serde_json::Value::as_str)
        .and_then(sanitize)
    {
        target.insert(
            field.to_string(),
            serde_json::Value::String(value.to_string()),
        );
    }
}

fn sanitize_candidate_phase(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "3c_trial" => Some("3c_trial"),
        "provider_request" => Some("provider_request"),
        _ => None,
    }
}

fn sanitize_candidate_api_format(value: &str) -> Option<&'static str> {
    match aether_ai_formats::normalize_api_format_alias(value).as_str() {
        "openai:chat" => Some("openai:chat"),
        "openai:responses" => Some("openai:responses"),
        "openai:responses:compact" => Some("openai:responses:compact"),
        "openai:search" => Some("openai:search"),
        "openai:embedding" => Some("openai:embedding"),
        "openai:rerank" => Some("openai:rerank"),
        "openai:image" => Some("openai:image"),
        "openai:video" => Some("openai:video"),
        "claude:messages" | "anthropic:messages" => Some("claude:messages"),
        "gemini:generate_content" => Some("gemini:generate_content"),
        "gemini:interactions" => Some("gemini:interactions"),
        "gemini:embedding" => Some("gemini:embedding"),
        "gemini:files" => Some("gemini:files"),
        "gemini:video" => Some("gemini:video"),
        "jina:embedding" => Some("jina:embedding"),
        "jina:rerank" => Some("jina:rerank"),
        "doubao:embedding" => Some("doubao:embedding"),
        "aliyun:multimodal_embedding" => Some("aliyun:multimodal_embedding"),
        _ => None,
    }
}

fn sanitize_candidate_execution_strategy(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "gateway_affinity_forward" => Some("gateway_affinity_forward"),
        "raw_public_proxy" => Some("raw_public_proxy"),
        "local_same_format" => Some("local_same_format"),
        "local_cross_format" => Some("local_cross_format"),
        _ => None,
    }
}

fn sanitize_candidate_conversion_mode(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "none" => Some("none"),
        "request_only" => Some("request_only"),
        "response_only" => Some("response_only"),
        "bidirectional" => Some("bidirectional"),
        _ => None,
    }
}

fn sanitize_candidate_ranking_mode(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "fixedorder" | "fixed_order" => Some("FixedOrder"),
        "cacheaffinity" | "cache_affinity" => Some("CacheAffinity"),
        "loadbalance" | "load_balance" => Some("LoadBalance"),
        _ => None,
    }
}

fn sanitize_candidate_priority_mode(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "provider" => Some("Provider"),
        "globalkey" | "global_key" => Some("GlobalKey"),
        _ => None,
    }
}

fn sanitize_candidate_promotion_reason(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "cached_affinity" => Some("cached_affinity"),
        "local_tunnel" => Some("local_tunnel"),
        _ => None,
    }
}

fn sanitize_candidate_demotion_reason(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "cross_format" => Some("cross_format"),
        _ => None,
    }
}

fn sanitize_candidate_source(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "execution_runtime" => Some("execution_runtime"),
        "upstream_response" => Some("upstream_response"),
        "usage_routing_snapshot" => Some("usage_routing_snapshot"),
        _ => None,
    }
}

fn sanitize_candidate_execution_path(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "public_proxy_passthrough" => Some("public_proxy_passthrough"),
        "local_proxy_passthrough_removed" => Some("local_proxy_passthrough_removed"),
        "execution_runtime_sync" => Some("execution_runtime_sync"),
        "execution_runtime_stream" => Some("execution_runtime_stream"),
        "control_execute_sync" => Some("control_execute_sync"),
        "control_execute_stream" => Some("control_execute_stream"),
        "local_execution_runtime_miss" => Some("local_execution_runtime_miss"),
        "local_execution_planning_timeout" => Some("local_execution_planning_timeout"),
        "local_api_key_concurrency_limited" => Some("local_api_key_concurrency_limited"),
        "local_auth_denied" => Some("local_auth_denied"),
        "local_rate_limited" => Some("local_rate_limited"),
        "local_invalid_request" => Some("local_invalid_request"),
        "local_route_not_found" => Some("local_route_not_found"),
        "local_overloaded" => Some("local_overloaded"),
        "distributed_overloaded" => Some("distributed_overloaded"),
        "local_ai_public" => Some("local_ai_public"),
        "local_execution_loop_detected" => Some("local_execution_loop_detected"),
        "tunnel_affinity_forward" => Some("tunnel_affinity_forward"),
        "responses_websocket_bridge" => Some("responses_websocket_bridge"),
        _ => None,
    }
}

fn sanitize_candidate_upstream_response(value: &serde_json::Value) -> Option<serde_json::Value> {
    let object = value.as_object()?;
    let mut summary = serde_json::Map::new();
    insert_candidate_known_string(object, &mut summary, "source", sanitize_candidate_source);
    if let Some(status_code) = object
        .get("status_code")
        .and_then(serde_json::Value::as_u64)
        .filter(|status_code| *status_code <= u64::from(u16::MAX))
    {
        summary.insert(
            "status_code".to_string(),
            serde_json::Value::Number(status_code.into()),
        );
    }
    insert_candidate_known_string(
        object,
        &mut summary,
        "body_state",
        sanitize_candidate_body_state,
    );
    (!summary.is_empty()).then_some(serde_json::Value::Object(summary))
}

fn sanitize_candidate_body_state(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "none" => Some("none"),
        "inline" => Some("inline"),
        "reference" => Some("reference"),
        "truncated" => Some("truncated"),
        "disabled" => Some("disabled"),
        "unavailable" => Some("unavailable"),
        _ => None,
    }
}

fn sanitize_candidate_image_progress(value: &serde_json::Value) -> Option<serde_json::Value> {
    let object = value.as_object()?;
    let mut progress = serde_json::Map::new();
    insert_candidate_known_string(
        object,
        &mut progress,
        "phase",
        sanitize_candidate_image_progress_phase,
    );
    insert_candidate_known_string(
        object,
        &mut progress,
        "last_upstream_event",
        sanitize_candidate_image_upstream_event,
    );
    insert_candidate_known_string(
        object,
        &mut progress,
        "last_client_visible_event",
        sanitize_candidate_image_client_event,
    );
    for field in [
        "upstream_ttfb_ms",
        "upstream_sse_frame_count",
        "last_upstream_frame_at_unix_ms",
        "partial_image_count",
        "downstream_heartbeat_count",
        "last_downstream_heartbeat_at_unix_ms",
        "downstream_heartbeat_interval_ms",
    ] {
        insert_candidate_u64(object, &mut progress, field);
    }
    (!progress.is_empty()).then_some(serde_json::Value::Object(progress))
}

fn sanitize_candidate_image_progress_phase(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "upstream_connecting" => Some("upstream_connecting"),
        "upstream_streaming" => Some("upstream_streaming"),
        "upstream_completed" => Some("upstream_completed"),
        "failed" => Some("failed"),
        _ => None,
    }
}

fn sanitize_candidate_image_upstream_event(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "response.image_generation_call.partial_image" => {
            Some("response.image_generation_call.partial_image")
        }
        "response.completed" => Some("response.completed"),
        "response.failed" => Some("response.failed"),
        "response.error" => Some("response.error"),
        "error" => Some("error"),
        "done" => Some("done"),
        _ => None,
    }
}

fn sanitize_candidate_image_client_event(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "image_generation.partial_image" => Some("image_generation.partial_image"),
        "image_generation.completed" => Some("image_generation.completed"),
        "image_generation.failed" => Some("image_generation.failed"),
        _ => None,
    }
}

fn sanitize_candidate_pool_group_exhaustion(
    value: &serde_json::Value,
) -> Option<serde_json::Value> {
    let object = value.as_object()?;
    let mut exhaustion = serde_json::Map::new();
    for field in ["scanned_keys", "budget_scanned_keys"] {
        insert_candidate_u64(object, &mut exhaustion, field);
    }

    let mut sanitized_counts = serde_json::Map::new();
    if let Some(counts) = object
        .get("skip_reason_counts")
        .and_then(serde_json::Value::as_object)
    {
        for (reason, count) in counts {
            let Some(count) = count.as_u64() else {
                continue;
            };
            let Some(reason) = sanitize_request_candidate_skip_reason(Some(reason.clone())) else {
                continue;
            };
            let combined = sanitized_counts
                .get(&reason)
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default()
                .saturating_add(count);
            sanitized_counts.insert(reason, serde_json::Value::Number(combined.into()));
        }
    }
    if !sanitized_counts.is_empty() {
        exhaustion.insert(
            "skip_reason_counts".to_string(),
            serde_json::Value::Object(sanitized_counts),
        );
    }

    (!exhaustion.is_empty()).then_some(serde_json::Value::Object(exhaustion))
}

#[async_trait]
pub trait RequestCandidateWriteRepository: Send + Sync {
    async fn upsert(
        &self,
        candidate: UpsertRequestCandidateRecord,
    ) -> Result<StoredRequestCandidate, crate::DataLayerError>;

    async fn upsert_many(
        &self,
        candidates: Vec<UpsertRequestCandidateRecord>,
    ) -> Result<usize, crate::DataLayerError> {
        let mut persisted = 0usize;
        for candidate in candidates {
            self.upsert(candidate).await?;
            persisted = persisted.saturating_add(1);
        }
        Ok(persisted)
    }

    async fn delete_created_before(
        &self,
        created_before_unix_secs: u64,
        limit: usize,
    ) -> Result<usize, crate::DataLayerError>;
}

pub trait RequestCandidateRepository:
    RequestCandidateReadRepository + RequestCandidateWriteRepository + Send + Sync
{
}

impl<T> RequestCandidateRepository for T where
    T: RequestCandidateReadRepository + RequestCandidateWriteRepository + Send + Sync
{
}

pub fn request_candidate_lifecycle_would_regress(
    existing: RequestCandidateStatus,
    incoming: RequestCandidateStatus,
) -> bool {
    let existing_is_terminal = matches!(
        existing,
        RequestCandidateStatus::Success
            | RequestCandidateStatus::Failed
            | RequestCandidateStatus::Cancelled
            | RequestCandidateStatus::Skipped
    );

    existing_is_terminal && incoming != existing
        || existing == RequestCandidateStatus::Pending
            && matches!(
                incoming,
                RequestCandidateStatus::Available | RequestCandidateStatus::Unused
            )
        || existing == RequestCandidateStatus::Streaming
            && matches!(
                incoming,
                RequestCandidateStatus::Available
                    | RequestCandidateStatus::Unused
                    | RequestCandidateStatus::Pending
            )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        derive_request_candidate_final_status, request_candidate_lifecycle_would_regress,
        sanitize_request_candidate_error_type, sanitize_request_candidate_skip_reason,
        RequestCandidateFinalStatus, RequestCandidateStatus, StoredRequestCandidate,
        UpsertRequestCandidateRecord, REQUEST_CANDIDATE_ERROR_TYPES,
        REQUEST_CANDIDATE_ERROR_TYPE_ALIASES, REQUEST_CANDIDATE_SKIP_REASONS,
        UNCLASSIFIED_CANDIDATE_ERROR_TYPE, UNCLASSIFIED_CANDIDATE_SKIP_REASON,
    };

    fn candidate(
        id: &str,
        status: RequestCandidateStatus,
        status_code: Option<i32>,
    ) -> StoredRequestCandidate {
        StoredRequestCandidate::new(
            id.to_string(),
            "req-1".to_string(),
            None,
            None,
            None,
            None,
            0,
            0,
            None,
            None,
            None,
            status,
            None,
            false,
            status_code,
            None,
            None,
            Some(100),
            None,
            None,
            None,
            1_700_000_000_000,
            Some(1_700_000_000_000),
            Some(1_700_000_000_100),
        )
        .expect("candidate should build")
    }

    #[test]
    fn failed_candidate_with_http_200_stays_final_failed() {
        let candidates = vec![candidate(
            "cand-1",
            RequestCandidateStatus::Failed,
            Some(200),
        )];

        assert_eq!(
            derive_request_candidate_final_status(&candidates),
            RequestCandidateFinalStatus::Failed
        );
    }

    #[test]
    fn explicit_success_candidate_still_wins_after_failed_attempt() {
        let candidates = vec![
            candidate("cand-1", RequestCandidateStatus::Failed, Some(503)),
            candidate("cand-2", RequestCandidateStatus::Success, Some(200)),
        ];

        assert_eq!(
            derive_request_candidate_final_status(&candidates),
            RequestCandidateFinalStatus::Success
        );
    }

    #[test]
    fn streaming_candidate_cannot_regress_to_an_earlier_planning_state() {
        for incoming in [
            RequestCandidateStatus::Available,
            RequestCandidateStatus::Unused,
            RequestCandidateStatus::Pending,
        ] {
            assert!(request_candidate_lifecycle_would_regress(
                RequestCandidateStatus::Streaming,
                incoming,
            ));
        }
        assert!(!request_candidate_lifecycle_would_regress(
            RequestCandidateStatus::Streaming,
            RequestCandidateStatus::Success,
        ));
    }

    #[test]
    fn pending_candidate_cannot_regress_to_an_earlier_planning_state() {
        for incoming in [
            RequestCandidateStatus::Available,
            RequestCandidateStatus::Unused,
        ] {
            assert!(request_candidate_lifecycle_would_regress(
                RequestCandidateStatus::Pending,
                incoming,
            ));
        }
        for incoming in [
            RequestCandidateStatus::Streaming,
            RequestCandidateStatus::Success,
        ] {
            assert!(!request_candidate_lifecycle_would_regress(
                RequestCandidateStatus::Pending,
                incoming,
            ));
        }
    }

    #[test]
    fn terminal_candidate_cannot_be_rewritten_to_a_different_terminal_fact() {
        for existing in [
            RequestCandidateStatus::Success,
            RequestCandidateStatus::Failed,
            RequestCandidateStatus::Cancelled,
            RequestCandidateStatus::Skipped,
        ] {
            for incoming in [
                RequestCandidateStatus::Success,
                RequestCandidateStatus::Failed,
                RequestCandidateStatus::Cancelled,
                RequestCandidateStatus::Skipped,
            ] {
                assert_eq!(
                    request_candidate_lifecycle_would_regress(existing, incoming),
                    existing != incoming,
                    "first terminal fact must win: {existing:?} -> {incoming:?}",
                );
            }
        }
    }

    #[test]
    fn candidate_upsert_rejects_nul_in_persistence_identity_fields() {
        let mut record = UpsertRequestCandidateRecord {
            id: "candidate-1".to_string(),
            request_id: "request-1".to_string(),
            user_id: None,
            api_key_id: None,
            username: None,
            api_key_name: None,
            candidate_index: 0,
            retry_index: 0,
            provider_id: None,
            endpoint_id: None,
            key_id: None,
            status: RequestCandidateStatus::Pending,
            skip_reason: None,
            is_cached: None,
            status_code: None,
            error_type: None,
            error_message: None,
            latency_ms: None,
            concurrent_requests: None,
            extra_data: None,
            required_capabilities: None,
            created_at_unix_ms: None,
            started_at_unix_ms: None,
            finished_at_unix_ms: None,
        };

        record.request_id = "request\0poison".to_string();
        assert!(record.validate().is_err());
        record.request_id = "request-1".to_string();
        record.key_id = Some("key\0poison".to_string());
        assert!(record.validate().is_err());
    }

    #[test]
    fn candidate_persistence_keeps_admin_errors_but_removes_request_credentials() {
        let mut record = UpsertRequestCandidateRecord {
            id: "candidate-1".to_string(),
            request_id: "request-1".to_string(),
            user_id: None,
            api_key_id: None,
            username: Some("alice-sensitive-display".to_string()),
            api_key_name: Some("production-sensitive-label".to_string()),
            candidate_index: 0,
            retry_index: 0,
            provider_id: None,
            endpoint_id: None,
            key_id: None,
            status: RequestCandidateStatus::Failed,
            skip_reason: None,
            is_cached: None,
            status_code: Some(401),
            error_type: Some("upstream_error".to_string()),
            error_message: Some("unauthorized".to_string()),
            latency_ms: None,
            concurrent_requests: None,
            extra_data: Some(json!({
                "upstream_url": "https://user:pass@example.com/v1/models?key=vertex-secret#fragment",
                "request_path_and_query": "/v1/models?key=client-secret&alt=sse",
                "key_name": "credential-label-secret",
                "header_rules": [{
                    "id": "header-rule-secret",
                    "action": "set",
                    "name": "x-auth",
                    "value": "secret",
                    "condition": {"path": "$.tenant_secret"}
                }],
                "body_rules": [{
                    "id": "body-rule-secret",
                    "action": "replace",
                    "path": "$.api_key",
                    "pattern": "secret-pattern",
                    "replacement": "secret"
                }],
                "proxy": {
                    "mode": "manual",
                    "source": "endpoint",
                    "url": "https://user:pass@proxy.example/private-secret?token=secret",
                    "ttfb_ms": 17
                },
                "routing_trace": {
                    "selection_source": "system_default",
                    "selected_rules": ["tenant-secret"],
                    "global_candidates": [{"key_id": "secret-key"}],
                    "client_request_patch_summary": {
                        "body_paths": ["$.secret"],
                        "header_names": ["authorization"]
                    }
                },
                "error_flow": {
                    "stage": "upstream",
                    "source": "upstream_response",
                    "classification": "retry_status_code",
                    "decision": "retry_next_candidate",
                    "propagation": "captured",
                    "retryable": true,
                    "status_code": 401,
                    "message": "token vertex-secret rejected"
                },
                "unknown": {"message": "secret"},
                "free_text": "Bearer secret",
                "gateway_execution_runtime": true,
                "client_api_format": "OPENAI:RESPONSES",
                "provider_api_format": "claude:messages",
                "execution_strategy": "local_cross_format",
                "conversion_mode": "bidirectional",
                "ranking_mode": "CacheAffinity",
                "priority_mode": "Provider",
                "ranking_index": 2,
                "priority_slot": 7,
                "promoted_by": "cached_affinity",
                "demoted_by": "cross_format",
                "upstream_response": {
                    "source": "upstream_response",
                    "status_code": 401,
                    "headers": {"set-cookie": "session=secret"},
                    "body": {"error": {"message": "token vertex-secret rejected"}},
                    "body_state": "inline"
                },
                "image_progress": {
                    "phase": "upstream_streaming",
                    "upstream_ttfb_ms": 20,
                    "upstream_sse_frame_count": 3,
                    "last_upstream_event": "response.image_generation_call.partial_image",
                    "last_upstream_frame_at_unix_ms": 1_700_000_000_100_u64,
                    "partial_image_count": 2,
                    "last_client_visible_event": "image_generation.partial_image",
                    "downstream_heartbeat_count": 4,
                    "last_downstream_heartbeat_at_unix_ms": 1_700_000_000_200_u64,
                    "downstream_heartbeat_interval_ms": 1_000,
                    "message": "secret"
                },
                "pool_group_exhaustion": {
                    "scanned_keys": 3,
                    "budget_scanned_keys": 2,
                    "skip_reason_counts": {
                        "pool_cooldown": 2,
                        "secret reason": 1
                    },
                    "message": "secret"
                }
            })),
            required_capabilities: Some(json!({
                "cache_1h": "TRUE",
                "context_1m": 1,
                "gemini_files": 0,
                "streaming": "false",
                "vision": true,
                "tenant_secret_capability": "Bearer secret",
                "billing": {"account": "secret-account"}
            })),
            created_at_unix_ms: None,
            started_at_unix_ms: None,
            finished_at_unix_ms: None,
        };

        record.sanitize_for_persistence();
        let sanitized_once = record.clone();
        record.sanitize_for_persistence();
        assert_eq!(
            record, sanitized_once,
            "candidate sanitization must be idempotent"
        );
        assert!(record.username.is_none());
        assert!(record.api_key_name.is_none());
        assert_eq!(record.error_message.as_deref(), Some("unauthorized"));
        let extra = record
            .extra_data
            .as_ref()
            .expect("safe candidate data should remain");
        for field in ["request_path_and_query", "key_name", "unknown", "free_text"] {
            assert!(extra.get(field).is_none(), "{field} must not be persisted");
        }
        assert_eq!(extra["upstream_url"], "https://example.com/");
        assert_eq!(extra["header_rules"]["count"], 1);
        assert_eq!(extra["header_rules"]["enabled_count"], 1);
        assert_eq!(extra["header_rules"]["conditional_count"], 1);
        assert_eq!(extra["header_rules"]["action_counts"]["set"], 1);
        assert_eq!(extra["body_rules"]["count"], 1);
        assert_eq!(extra["body_rules"]["action_counts"]["replace"], 1);
        assert_eq!(extra["proxy"]["mode"], "manual");
        assert_eq!(extra["proxy"]["source"], "endpoint");
        assert_eq!(extra["proxy"]["url"], "https://proxy.example/");
        assert_eq!(extra["proxy"]["ttfb_ms"], 17);
        assert_eq!(extra["routing_trace"]["selection_source"], "system_default");
        assert_eq!(extra["routing_trace"]["selected_rule_count"], 1);
        assert_eq!(extra["routing_trace"]["global_candidate_count"], 1);
        assert_eq!(
            extra["routing_trace"]["client_request_patch_summary"]["body_patch_count"],
            1
        );
        assert_eq!(
            extra["routing_trace"]["client_request_patch_summary"]["header_patch_count"],
            1
        );
        assert_eq!(extra["error_flow"]["stage"], "upstream");
        assert_eq!(extra["error_flow"]["retryable"], true);
        assert_eq!(extra["error_flow"]["status_code"], 401);
        assert_eq!(
            extra["error_flow"]["message"],
            "token vertex-secret rejected"
        );
        assert_eq!(extra["gateway_execution_runtime"], true);
        assert_eq!(extra["client_api_format"], "openai:responses");
        assert_eq!(extra["provider_api_format"], "claude:messages");
        assert_eq!(extra["execution_strategy"], "local_cross_format");
        assert_eq!(extra["conversion_mode"], "bidirectional");
        assert_eq!(extra["ranking_mode"], "CacheAffinity");
        assert_eq!(extra["priority_mode"], "Provider");
        assert_eq!(extra["ranking_index"], 2);
        assert_eq!(extra["priority_slot"], 7);
        assert_eq!(extra["promoted_by"], "cached_affinity");
        assert_eq!(extra["demoted_by"], "cross_format");
        assert_eq!(extra["upstream_response"]["source"], "upstream_response");
        assert_eq!(extra["upstream_response"]["status_code"], 401);
        assert_eq!(extra["upstream_response"]["body_state"], "inline");
        assert_eq!(
            extra["upstream_response"]["headers"]["set-cookie"],
            "session=secret"
        );
        assert_eq!(
            extra["upstream_response"]["body"]["error"]["message"],
            "token vertex-secret rejected"
        );
        assert_eq!(
            extra["image_progress"]["last_client_visible_event"],
            "image_generation.partial_image"
        );
        assert_eq!(extra["image_progress"]["phase"], "upstream_streaming");
        assert_eq!(extra["image_progress"]["upstream_ttfb_ms"], 20);
        assert_eq!(extra["image_progress"]["upstream_sse_frame_count"], 3);
        assert_eq!(
            extra["image_progress"]["last_upstream_event"],
            "response.image_generation_call.partial_image"
        );
        assert_eq!(
            extra["image_progress"]["last_upstream_frame_at_unix_ms"],
            1_700_000_000_100_u64
        );
        assert_eq!(extra["image_progress"]["partial_image_count"], 2);
        assert_eq!(extra["image_progress"]["downstream_heartbeat_count"], 4);
        assert_eq!(
            extra["image_progress"]["last_downstream_heartbeat_at_unix_ms"],
            1_700_000_000_200_u64
        );
        assert_eq!(
            extra["image_progress"]["downstream_heartbeat_interval_ms"],
            1_000
        );
        assert!(extra["image_progress"].get("message").is_none());
        assert_eq!(extra["pool_group_exhaustion"]["scanned_keys"], 3);
        assert_eq!(
            extra["pool_group_exhaustion"]["skip_reason_counts"]["pool_cooldown"],
            2
        );
        assert_eq!(
            extra["pool_group_exhaustion"]["skip_reason_counts"]["unclassified_skip"],
            1
        );
        assert!(extra["pool_group_exhaustion"].get("message").is_none());
        assert_eq!(
            record.required_capabilities,
            Some(json!({
                "cache_1h": true,
                "context_1m": true,
                "gemini_files": false,
                "streaming": false,
                "vision": true
            }))
        );

        let serialized = serde_json::to_string(&record).expect("candidate should serialize");
        for sensitive in [
            "client-secret",
            "credential-label-secret",
            "header-rule-secret",
            "body-rule-secret",
            "x-auth",
            "$.api_key",
            "secret-pattern",
            "tenant-secret",
            "secret-key",
            "authorization",
            "tenant_secret_capability",
            "secret-account",
            "alice-sensitive-display",
            "production-sensitive-label",
        ] {
            assert!(
                !serialized.contains(sensitive),
                "candidate must not retain {sensitive}"
            );
        }
        let public_extra = super::sanitize_request_candidate_extra_data(record.extra_data);
        let serialized =
            serde_json::to_string(&public_extra).expect("public data should serialize");
        assert!(!serialized.contains("vertex-secret"));
        assert!(!serialized.contains("session=secret"));
    }

    #[test]
    fn candidate_persistence_keeps_only_known_diagnostic_categories() {
        let mut record = UpsertRequestCandidateRecord {
            id: "candidate-1".to_string(),
            request_id: "request-1".to_string(),
            user_id: None,
            api_key_id: None,
            username: None,
            api_key_name: None,
            candidate_index: 0,
            retry_index: 0,
            provider_id: None,
            endpoint_id: None,
            key_id: None,
            status: RequestCandidateStatus::Failed,
            skip_reason: Some("Provider auth failed with token sk-secret".to_string()),
            is_cached: None,
            status_code: Some(401),
            error_type: Some("sk_secret_looks_like_a_category".to_string()),
            error_message: None,
            latency_ms: None,
            concurrent_requests: None,
            extra_data: None,
            required_capabilities: None,
            created_at_unix_ms: None,
            started_at_unix_ms: None,
            finished_at_unix_ms: None,
        };

        record.sanitize_for_persistence();
        assert_eq!(
            record.skip_reason.as_deref(),
            Some(UNCLASSIFIED_CANDIDATE_SKIP_REASON)
        );
        assert_eq!(
            record.error_type.as_deref(),
            Some(UNCLASSIFIED_CANDIDATE_ERROR_TYPE)
        );

        record.skip_reason = Some(" Pool_Cooldown ".to_string());
        record.error_type = Some(" FirstByteTimeout ".to_string());
        record.sanitize_for_persistence();
        assert_eq!(record.skip_reason.as_deref(), Some("pool_cooldown"));
        assert_eq!(record.error_type.as_deref(), Some("first_byte_timeout"));

        record.skip_reason = Some("Pool_Score_Member_Missing".to_string());
        record.sanitize_for_persistence();
        assert_eq!(
            record.skip_reason.as_deref(),
            Some("pool_score_member_missing")
        );

        record.error_type = Some("Upstream5xx".to_string());
        record.sanitize_for_persistence();
        assert_eq!(record.error_type.as_deref(), Some("upstream5xx"));

        for reason in REQUEST_CANDIDATE_SKIP_REASONS {
            assert_eq!(
                sanitize_request_candidate_skip_reason(Some((*reason).to_string())).as_deref(),
                Some(*reason)
            );
        }
        for error_type in REQUEST_CANDIDATE_ERROR_TYPES {
            assert_eq!(
                sanitize_request_candidate_error_type(Some((*error_type).to_string())).as_deref(),
                Some(*error_type)
            );
        }
        for (alias, canonical) in REQUEST_CANDIDATE_ERROR_TYPE_ALIASES {
            assert_eq!(
                sanitize_request_candidate_error_type(Some((*alias).to_string())).as_deref(),
                Some(*canonical)
            );
        }
    }

    #[test]
    fn candidate_database_read_preserves_errors_until_public_projection() {
        let mut candidate = StoredRequestCandidate::new(
            "candidate-1".to_string(),
            "request-1".to_string(),
            None,
            None,
            Some("legacy-sensitive-user".to_string()),
            Some("legacy-sensitive-key-label".to_string()),
            0,
            0,
            None,
            None,
            None,
            RequestCandidateStatus::Failed,
            Some("legacy secret in skip reason".to_string()),
            false,
            Some(500),
            Some("legacy_secret_code".to_string()),
            Some("legacy secret message".to_string()),
            None,
            None,
            None,
            None,
            1,
            None,
            Some(2),
        )
        .expect("candidate should build");

        assert_eq!(
            candidate.skip_reason.as_deref(),
            Some(UNCLASSIFIED_CANDIDATE_SKIP_REASON)
        );
        assert_eq!(
            candidate.error_type.as_deref(),
            Some(UNCLASSIFIED_CANDIDATE_ERROR_TYPE)
        );
        assert_eq!(
            candidate.error_message.as_deref(),
            Some("legacy secret message")
        );
        assert!(candidate.username.is_none());
        assert!(candidate.api_key_name.is_none());
        candidate.sanitize_sensitive_diagnostics();
        assert!(candidate.error_message.is_none());
    }

    #[test]
    fn admin_diagnostics_are_bounded_and_public_projection_removes_them() {
        let raw = json!({
            "upstream_response": {"status_code": 400, "body": "错误内容".repeat(20_000)},
            "error_flow": {"status_code": 400, "message": "private upstream failure"},
            "failure_diagnostic": {"path": "$.input", "message": "private conversion failure", "safe_to_show": false, "stage": "request", "details": {"code": "invalid_enum_value", "actual": "private-value"}},
            "request_body": {"input": "private prompt"}
        });
        let admin = super::sanitize_request_candidate_extra_data_for_persistence(Some(raw))
            .expect("admin diagnostics should remain");
        let body = admin["upstream_response"]["body"]
            .as_str()
            .expect("body should be text");
        assert!(body.len() <= 65_536);
        assert!(body.ends_with("...[truncated]"));
        assert!(admin.get("request_body").is_none());
        assert_eq!(admin["failure_diagnostic"]["safe_to_show"], false);
        assert_eq!(admin["failure_diagnostic"]["stage"], "request");
        assert_eq!(
            admin["failure_diagnostic"]["details"]["code"],
            "invalid_enum_value"
        );
        assert_eq!(
            super::sanitize_request_candidate_extra_data_for_persistence(Some(admin.clone())),
            Some(admin.clone()),
        );
        let public = super::sanitize_request_candidate_extra_data(Some(admin))
            .expect("public status should remain");
        assert_eq!(public["upstream_response"]["status_code"], 400);
        assert!(public["upstream_response"].get("body").is_none());
        assert!(public["error_flow"].get("message").is_none());
        assert!(public.get("failure_diagnostic").is_none());
    }

    #[test]
    fn decision_trace_candidate_sanitizes_catalog_enrichment_and_is_idempotent() {
        let mut stored = candidate("candidate-1", RequestCandidateStatus::Failed, Some(500));
        stored.error_message = Some("Bearer candidate-secret".to_string());
        stored.extra_data = Some(json!({
            "gateway_execution_runtime": true,
            "request_body": {"password": "candidate-secret"}
        }));
        stored.required_capabilities = Some(json!({
            "VISION": 1,
            "tenant_secret": "candidate-secret"
        }));
        let mut item = super::DecisionTraceCandidate {
            candidate: stored,
            provider_name: Some("Provider".to_string()),
            provider_website: Some(
                "https://user:pass@example.com/private/tenant-secret?token=secret#fragment"
                    .to_string(),
            ),
            provider_type: Some("custom".to_string()),
            provider_priority: Some(1),
            provider_keep_priority_on_conversion: Some(false),
            provider_enable_format_conversion: Some(true),
            endpoint_api_format: Some("openai:chat".to_string()),
            endpoint_api_family: Some("openai".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_format_acceptance_config: Some(json!({
                "secret_pattern": "tenant-secret"
            })),
            provider_key_name: Some("prod".to_string()),
            provider_key_auth_type: Some("api_key".to_string()),
            provider_key_api_formats: Some(json!([
                "OPENAI:CHAT",
                "anthropic:messages",
                "tenant-secret-format"
            ])),
            provider_key_internal_priority: Some(5),
            provider_key_global_priority_by_format: Some(json!({
                "tenant-secret-format": 1
            })),
            provider_key_capabilities: Some(json!({
                "cache_1h": "TRUE",
                "tenant_secret": "candidate-secret"
            })),
            provider_key_is_active: Some(true),
        };

        item.sanitize_sensitive_diagnostics();
        let sanitized_once = item.clone();
        item.sanitize_sensitive_diagnostics();

        assert_eq!(item, sanitized_once);
        assert_eq!(
            item.provider_website.as_deref(),
            Some("https://example.com/")
        );
        assert!(item.endpoint_format_acceptance_config.is_none());
        assert_eq!(
            item.provider_key_api_formats,
            Some(json!(["openai:chat", "claude:messages"]))
        );
        assert!(item.provider_key_global_priority_by_format.is_none());
        assert_eq!(
            item.provider_key_capabilities,
            Some(json!({"cache_1h": true}))
        );
        assert!(item.candidate.error_message.is_none());
        assert_eq!(
            item.candidate.extra_data,
            Some(json!({"gateway_execution_runtime": true}))
        );
        assert_eq!(
            item.candidate.required_capabilities,
            Some(json!({"vision": true}))
        );
        let serialized = serde_json::to_string(&item).expect("trace candidate should serialize");
        assert!(!serialized.contains("candidate-secret"));
        assert!(!serialized.contains("tenant-secret"));
        assert!(!serialized.contains("user:pass"));
    }
}
