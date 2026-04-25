use std::sync::Arc;

use aether_provider_transport::provider_types::provider_type_is_fixed;
use tracing::warn;

use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;

use crate::ai_pipeline::{GatewayProviderTransportSnapshot, PlannerAppState};
use crate::orchestration::LocalExecutionCandidateMetadata;

use super::candidate_affinity::rank_eligible_local_execution_candidates;
use super::pool_scheduler::apply_local_execution_pool_scheduler;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EligibleLocalExecutionCandidate {
    pub(crate) candidate: SchedulerMinimalCandidateSelectionCandidate,
    pub(crate) transport: Arc<GatewayProviderTransportSnapshot>,
    pub(crate) provider_api_format: String,
    pub(crate) orchestration: LocalExecutionCandidateMetadata,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SkippedLocalExecutionCandidate {
    pub(crate) candidate: SchedulerMinimalCandidateSelectionCandidate,
    pub(crate) skip_reason: &'static str,
    pub(crate) transport: Option<Arc<GatewayProviderTransportSnapshot>>,
    pub(crate) extra_data: Option<serde_json::Value>,
}

impl SkippedLocalExecutionCandidate {
    pub(crate) fn transport_ref(&self) -> Option<&GatewayProviderTransportSnapshot> {
        self.transport.as_deref()
    }
}

pub(crate) async fn filter_and_rank_local_execution_candidates(
    state: PlannerAppState<'_>,
    candidates: Vec<SchedulerMinimalCandidateSelectionCandidate>,
    client_api_format: &str,
    requested_model: &str,
    required_capabilities: Option<&serde_json::Value>,
    sticky_session_token: Option<&str>,
) -> (
    Vec<EligibleLocalExecutionCandidate>,
    Vec<SkippedLocalExecutionCandidate>,
) {
    let requested_model = requested_model.trim();
    filter_and_rank_local_execution_candidates_with_gate(
        state,
        candidates,
        client_api_format,
        required_capabilities,
        sticky_session_token,
        |candidate, transport, normalized_client_api_format| {
            current_local_execution_candidate_skip_reason_with_transport(
                candidate,
                transport,
                normalized_client_api_format,
                requested_model,
            )
        },
    )
    .await
}

pub(crate) async fn filter_and_rank_local_execution_candidates_without_transport_pair_gate(
    state: PlannerAppState<'_>,
    candidates: Vec<SchedulerMinimalCandidateSelectionCandidate>,
    client_api_format: &str,
    requested_model: Option<&str>,
    required_capabilities: Option<&serde_json::Value>,
    sticky_session_token: Option<&str>,
) -> (
    Vec<EligibleLocalExecutionCandidate>,
    Vec<SkippedLocalExecutionCandidate>,
) {
    let requested_model = requested_model.map(str::trim);
    filter_and_rank_local_execution_candidates_with_gate(
        state,
        candidates,
        client_api_format,
        required_capabilities,
        sticky_session_token,
        |candidate, transport, _normalized_client_api_format| {
            current_local_execution_candidate_common_skip_reason_with_transport(
                candidate,
                transport,
                requested_model,
            )
        },
    )
    .await
}

async fn filter_and_rank_local_execution_candidates_with_gate<F>(
    state: PlannerAppState<'_>,
    candidates: Vec<SchedulerMinimalCandidateSelectionCandidate>,
    client_api_format: &str,
    required_capabilities: Option<&serde_json::Value>,
    sticky_session_token: Option<&str>,
    runtime_skip_reason: F,
) -> (
    Vec<EligibleLocalExecutionCandidate>,
    Vec<SkippedLocalExecutionCandidate>,
)
where
    F: Fn(
        &SchedulerMinimalCandidateSelectionCandidate,
        &GatewayProviderTransportSnapshot,
        &str,
    ) -> Option<&'static str>,
{
    let normalized_client_api_format = client_api_format.trim().to_ascii_lowercase();
    let mut selectable = Vec::with_capacity(candidates.len());
    let mut skipped = Vec::with_capacity(candidates.len());

    for candidate in candidates {
        let Some(transport) = read_candidate_transport_snapshot(state, &candidate).await else {
            skipped.push(SkippedLocalExecutionCandidate {
                candidate,
                skip_reason: "transport_snapshot_missing",
                transport: None,
                extra_data: None,
            });
            continue;
        };
        let transport = Arc::new(transport);
        if candidate_is_ineligible_due_to_disabled_format_conversion(
            transport.as_ref(),
            normalized_client_api_format.as_str(),
        ) {
            continue;
        }
        match runtime_skip_reason(
            &candidate,
            transport.as_ref(),
            normalized_client_api_format.as_str(),
        ) {
            Some(skip_reason) => skipped.push(SkippedLocalExecutionCandidate {
                candidate,
                skip_reason,
                transport: Some(transport),
                extra_data: None,
            }),
            None => selectable.push(EligibleLocalExecutionCandidate {
                provider_api_format: transport.endpoint.api_format.trim().to_ascii_lowercase(),
                candidate,
                transport,
                orchestration: LocalExecutionCandidateMetadata::default(),
            }),
        }
    }

    let ranked = rank_eligible_local_execution_candidates(
        state,
        selectable,
        normalized_client_api_format.as_str(),
        required_capabilities,
    )
    .await;
    let (ranked, pool_skipped) =
        apply_local_execution_pool_scheduler(state, ranked, sticky_session_token).await;
    skipped.extend(pool_skipped);

    (ranked, skipped)
}

pub(crate) fn extract_pool_sticky_session_token(body_json: &serde_json::Value) -> Option<String> {
    fn non_empty_str(value: Option<&serde_json::Value>) -> Option<&str> {
        value
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    let object = body_json.as_object()?;

    non_empty_str(object.get("prompt_cache_key"))
        .or_else(|| non_empty_str(object.get("conversation_id")))
        .or_else(|| non_empty_str(object.get("conversationId")))
        .or_else(|| non_empty_str(object.get("session_id")))
        .or_else(|| non_empty_str(object.get("sessionId")))
        .or_else(|| {
            object
                .get("metadata")
                .and_then(serde_json::Value::as_object)
                .and_then(|metadata| {
                    non_empty_str(metadata.get("session_id"))
                        .or_else(|| non_empty_str(metadata.get("conversation_id")))
                })
        })
        .or_else(|| {
            object
                .get("conversationState")
                .and_then(serde_json::Value::as_object)
                .and_then(|state| {
                    non_empty_str(state.get("conversationId"))
                        .or_else(|| non_empty_str(state.get("sessionId")))
                })
        })
        .map(ToOwned::to_owned)
}

fn current_local_execution_candidate_common_skip_reason_with_transport(
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    transport: &GatewayProviderTransportSnapshot,
    requested_model: Option<&str>,
) -> Option<&'static str> {
    let requested_model = requested_model.unwrap_or_default();

    if !transport.provider.is_active {
        return Some("provider_inactive");
    }
    if !transport.endpoint.is_active {
        return Some("endpoint_inactive");
    }
    if !transport.key.is_active {
        return Some("key_inactive");
    }

    let endpoint_api_format = transport.endpoint.api_format.trim();
    if !candidate
        .endpoint_api_format
        .trim()
        .eq_ignore_ascii_case(endpoint_api_format)
    {
        return Some("endpoint_api_format_changed");
    }

    if !transport_key_supports_api_format(transport, endpoint_api_format) {
        return Some("key_api_format_disabled");
    }
    if !transport_key_allows_candidate_model(transport, requested_model, candidate) {
        return Some("key_model_disabled");
    }

    None
}

fn candidate_is_ineligible_due_to_disabled_format_conversion(
    transport: &GatewayProviderTransportSnapshot,
    normalized_client_api_format: &str,
) -> bool {
    let endpoint_api_format = transport.endpoint.api_format.trim();
    if endpoint_api_format.eq_ignore_ascii_case(normalized_client_api_format) {
        return false;
    }

    crate::ai_pipeline::conversion::request_conversion_kind(
        normalized_client_api_format,
        endpoint_api_format,
    )
    .is_some()
        && crate::ai_pipeline::conversion::request_conversion_requires_enable_flag(
            normalized_client_api_format,
            endpoint_api_format,
        )
        && !crate::ai_pipeline::conversion::request_conversion_enabled_for_transport(
            transport,
            normalized_client_api_format,
            endpoint_api_format,
        )
}

fn current_local_execution_candidate_skip_reason_with_transport(
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    transport: &GatewayProviderTransportSnapshot,
    normalized_client_api_format: &str,
    requested_model: &str,
) -> Option<&'static str> {
    if let Some(skip_reason) = current_local_execution_candidate_common_skip_reason_with_transport(
        candidate,
        transport,
        Some(requested_model),
    ) {
        return Some(skip_reason);
    }

    let endpoint_api_format = transport.endpoint.api_format.trim();
    if endpoint_api_format.eq_ignore_ascii_case(normalized_client_api_format) {
        return None;
    }

    if !crate::ai_pipeline::conversion::request_pair_allowed_for_transport(
        transport,
        normalized_client_api_format,
        endpoint_api_format,
    ) {
        return Some("transport_unsupported");
    }

    None
}

fn transport_key_supports_api_format(
    transport: &GatewayProviderTransportSnapshot,
    endpoint_api_format: &str,
) -> bool {
    let provider_type = transport.provider.provider_type.trim();
    let auth_type = transport.key.auth_type.trim();
    let inherits_provider_api_formats = provider_type_is_fixed(provider_type)
        && (auth_type.eq_ignore_ascii_case("oauth")
            || (provider_type.eq_ignore_ascii_case("kiro")
                && auth_type.eq_ignore_ascii_case("bearer")
                && transport
                    .key
                    .decrypted_auth_config
                    .as_deref()
                    .map(str::trim)
                    .is_some_and(|value| !value.is_empty())));
    if inherits_provider_api_formats {
        return true;
    }

    match transport.key.api_formats.as_deref() {
        None => true,
        Some(formats) => formats
            .iter()
            .any(|value| value.trim().eq_ignore_ascii_case(endpoint_api_format)),
    }
}

fn transport_key_allows_candidate_model(
    transport: &GatewayProviderTransportSnapshot,
    requested_model: &str,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
) -> bool {
    let Some(allowed_models) = transport.key.allowed_models.as_deref() else {
        return true;
    };

    let requested_model = requested_model.trim();
    let global_model_name = candidate.global_model_name.trim();
    let selected_provider_model_name = candidate.selected_provider_model_name.trim();
    let mapping_matched_model = candidate
        .mapping_matched_model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    for allowed_model in allowed_models.iter().map(String::as_str).map(str::trim) {
        if allowed_model.is_empty() {
            continue;
        }
        if allowed_model == requested_model
            || allowed_model == global_model_name
            || allowed_model == selected_provider_model_name
            || mapping_matched_model.is_some_and(|value| value == allowed_model)
        {
            return true;
        }
    }

    false
}

pub(crate) async fn read_candidate_transport_snapshot(
    state: PlannerAppState<'_>,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
) -> Option<GatewayProviderTransportSnapshot> {
    match state
        .read_provider_transport_snapshot(
            &candidate.provider_id,
            &candidate.endpoint_id,
            &candidate.key_id,
        )
        .await
    {
        Ok(Some(transport)) => Some(transport),
        Ok(None) => None,
        Err(error) => {
            warn!(
                event_name = "candidate_eligibility_transport_load_failed",
                log_type = "event",
                provider_id = %candidate.provider_id,
                endpoint_id = %candidate.endpoint_id,
                key_id = %candidate.key_id,
                error = ?error,
                "failed to load provider transport while evaluating local candidate eligibility"
            );
            None
        }
    }
}
