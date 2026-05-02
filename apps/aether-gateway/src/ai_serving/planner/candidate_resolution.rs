use std::sync::Arc;

use aether_ai_serving::{
    run_ai_candidate_resolution, AiCandidateResolutionMode, AiCandidateResolutionPort,
    AiCandidateResolutionRequest,
};
use async_trait::async_trait;
use std::convert::Infallible;
use tracing::warn;

use aether_scheduler_core::{SchedulerMinimalCandidateSelectionCandidate, SchedulerRankingOutcome};

use crate::ai_serving::{
    candidate_common_transport_skip_reason, candidate_transport_pair_skip_reason,
    CandidateTransportPolicyFacts, GatewayAuthApiKeySnapshot, GatewayProviderTransportSnapshot,
    PlannerAppState,
};
use crate::orchestration::LocalExecutionCandidateMetadata;

use super::candidate_ranking::rank_eligible_local_execution_candidates;
use super::pool_scheduler::apply_local_execution_pool_scheduler;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EligibleLocalExecutionCandidate {
    pub(crate) candidate: SchedulerMinimalCandidateSelectionCandidate,
    pub(crate) transport: Arc<GatewayProviderTransportSnapshot>,
    pub(crate) provider_api_format: String,
    pub(crate) orchestration: LocalExecutionCandidateMetadata,
    pub(crate) ranking: Option<SchedulerRankingOutcome>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SkippedLocalExecutionCandidate {
    pub(crate) candidate: SchedulerMinimalCandidateSelectionCandidate,
    pub(crate) skip_reason: &'static str,
    pub(crate) transport: Option<Arc<GatewayProviderTransportSnapshot>>,
    pub(crate) ranking: Option<SchedulerRankingOutcome>,
    pub(crate) extra_data: Option<serde_json::Value>,
}

impl SkippedLocalExecutionCandidate {
    pub(crate) fn transport_ref(&self) -> Option<&GatewayProviderTransportSnapshot> {
        self.transport.as_deref()
    }
}

struct GatewayLocalCandidateResolutionPort<'a> {
    state: PlannerAppState<'a>,
    requested_model: Option<&'a str>,
    auth_snapshot: Option<&'a GatewayAuthApiKeySnapshot>,
    required_capabilities: Option<&'a serde_json::Value>,
    sticky_session_token: Option<&'a str>,
    request_auth_channel: Option<&'a str>,
}

#[async_trait]
impl AiCandidateResolutionPort for GatewayLocalCandidateResolutionPort<'_> {
    type Candidate = SchedulerMinimalCandidateSelectionCandidate;
    type Transport = GatewayProviderTransportSnapshot;
    type Eligible = EligibleLocalExecutionCandidate;
    type Skipped = SkippedLocalExecutionCandidate;
    type Error = Infallible;

    async fn read_candidate_transport(
        &self,
        candidate: &Self::Candidate,
    ) -> Result<Option<Self::Transport>, Self::Error> {
        Ok(read_candidate_transport_snapshot(self.state, candidate).await)
    }

    fn build_missing_transport_skipped_candidate(
        &self,
        candidate: Self::Candidate,
    ) -> Self::Skipped {
        SkippedLocalExecutionCandidate {
            candidate,
            skip_reason: "transport_snapshot_missing",
            transport: None,
            ranking: None,
            extra_data: None,
        }
    }

    fn candidate_common_skip_reason(
        &self,
        candidate: &Self::Candidate,
        transport: &Self::Transport,
        requested_model: Option<&str>,
    ) -> Option<&'static str> {
        if let Some(skip_reason) =
            candidate_auth_channel_skip_reason(transport, self.request_auth_channel)
        {
            return Some(skip_reason);
        }
        candidate_common_transport_skip_reason(
            transport,
            candidate_transport_policy_facts(candidate),
            requested_model,
        )
    }

    fn candidate_transport_pair_skip_reason(
        &self,
        candidate: &Self::Candidate,
        transport: &Self::Transport,
        normalized_client_api_format: &str,
        requested_model: &str,
    ) -> Option<&'static str> {
        let _ = (candidate, requested_model);
        candidate_transport_pair_skip_reason(transport, normalized_client_api_format)
    }

    fn build_skipped_candidate(
        &self,
        candidate: Self::Candidate,
        transport: Self::Transport,
        skip_reason: &'static str,
    ) -> Self::Skipped {
        SkippedLocalExecutionCandidate {
            candidate,
            skip_reason,
            transport: Some(Arc::new(transport)),
            ranking: None,
            extra_data: None,
        }
    }

    fn build_eligible_candidate(
        &self,
        candidate: Self::Candidate,
        transport: Self::Transport,
    ) -> Self::Eligible {
        let provider_api_format = transport.endpoint.api_format.trim().to_ascii_lowercase();
        EligibleLocalExecutionCandidate {
            candidate,
            transport: Arc::new(transport),
            provider_api_format,
            orchestration: LocalExecutionCandidateMetadata::default(),
            ranking: None,
        }
    }

    async fn rank_eligible_candidates(
        &self,
        candidates: Vec<Self::Eligible>,
        normalized_client_api_format: &str,
    ) -> Result<Vec<Self::Eligible>, Self::Error> {
        Ok(rank_eligible_local_execution_candidates(
            self.state,
            candidates,
            normalized_client_api_format,
            self.requested_model,
            self.auth_snapshot,
            self.required_capabilities,
        )
        .await)
    }

    async fn apply_pool_scheduler(
        &self,
        candidates: Vec<Self::Eligible>,
    ) -> Result<(Vec<Self::Eligible>, Vec<Self::Skipped>), Self::Error> {
        Ok(
            apply_local_execution_pool_scheduler(self.state, candidates, self.sticky_session_token)
                .await,
        )
    }
}

pub(crate) async fn resolve_and_rank_local_execution_candidates(
    state: PlannerAppState<'_>,
    candidates: Vec<SchedulerMinimalCandidateSelectionCandidate>,
    client_api_format: &str,
    requested_model: &str,
    auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
    required_capabilities: Option<&serde_json::Value>,
    sticky_session_token: Option<&str>,
    request_auth_channel: Option<&str>,
) -> (
    Vec<EligibleLocalExecutionCandidate>,
    Vec<SkippedLocalExecutionCandidate>,
) {
    let requested_model = requested_model.trim();
    resolve_and_rank_local_execution_candidates_with_mode(
        state,
        candidates,
        client_api_format,
        Some(requested_model),
        auth_snapshot,
        required_capabilities,
        sticky_session_token,
        request_auth_channel,
        AiCandidateResolutionMode::Standard,
    )
    .await
}

pub(crate) async fn resolve_and_rank_local_execution_candidates_without_transport_pair_gate(
    state: PlannerAppState<'_>,
    candidates: Vec<SchedulerMinimalCandidateSelectionCandidate>,
    client_api_format: &str,
    requested_model: Option<&str>,
    auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
    required_capabilities: Option<&serde_json::Value>,
    sticky_session_token: Option<&str>,
    request_auth_channel: Option<&str>,
) -> (
    Vec<EligibleLocalExecutionCandidate>,
    Vec<SkippedLocalExecutionCandidate>,
) {
    let requested_model = requested_model.map(str::trim);
    resolve_and_rank_local_execution_candidates_with_mode(
        state,
        candidates,
        client_api_format,
        requested_model,
        auth_snapshot,
        required_capabilities,
        sticky_session_token,
        request_auth_channel,
        AiCandidateResolutionMode::WithoutTransportPairGate,
    )
    .await
}

async fn resolve_and_rank_local_execution_candidates_with_mode(
    state: PlannerAppState<'_>,
    candidates: Vec<SchedulerMinimalCandidateSelectionCandidate>,
    client_api_format: &str,
    requested_model: Option<&str>,
    auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
    required_capabilities: Option<&serde_json::Value>,
    sticky_session_token: Option<&str>,
    request_auth_channel: Option<&str>,
    mode: AiCandidateResolutionMode,
) -> (
    Vec<EligibleLocalExecutionCandidate>,
    Vec<SkippedLocalExecutionCandidate>,
) {
    let port = GatewayLocalCandidateResolutionPort {
        state,
        requested_model,
        auth_snapshot,
        required_capabilities,
        sticky_session_token,
        request_auth_channel,
    };

    let request = AiCandidateResolutionRequest {
        client_api_format,
        requested_model,
        mode,
    };

    match run_ai_candidate_resolution(&port, candidates, request).await {
        Ok(outcome) => (outcome.eligible_candidates, outcome.skipped_candidates),
        Err(error) => match error {},
    }
}

fn candidate_transport_policy_facts(
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
) -> CandidateTransportPolicyFacts<'_> {
    CandidateTransportPolicyFacts {
        endpoint_api_format: candidate.endpoint_api_format.as_str(),
        global_model_name: candidate.global_model_name.as_str(),
        selected_provider_model_name: candidate.selected_provider_model_name.as_str(),
        mapping_matched_model: candidate.mapping_matched_model.as_deref(),
    }
}

fn candidate_auth_channel_skip_reason(
    transport: &GatewayProviderTransportSnapshot,
    request_auth_channel: Option<&str>,
) -> Option<&'static str> {
    let request_auth_channel = normalize_request_auth_channel(request_auth_channel?)?;
    let upstream_auth_channel = resolve_transport_request_auth_channel(transport)?;
    if request_auth_channel == upstream_auth_channel
        || allow_auth_channel_mismatch_for_format(transport)
    {
        None
    } else {
        Some("auth_channel_mismatch")
    }
}

fn normalize_request_auth_channel(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "api_key" | "api-key" | "apikey" => Some("api_key"),
        "bearer_like" | "bearer-like" | "bearer" | "oauth" => Some("bearer_like"),
        _ => None,
    }
}

fn resolve_transport_request_auth_channel(
    transport: &GatewayProviderTransportSnapshot,
) -> Option<&'static str> {
    let auth_type = resolve_transport_auth_type_for_endpoint_format(transport);
    match auth_type.as_str() {
        "api_key" => Some("api_key"),
        "bearer" => Some("bearer_like"),
        "oauth" if provider_uses_bearer_like_oauth(&transport.provider.provider_type) => {
            Some("bearer_like")
        }
        _ => None,
    }
}

fn resolve_transport_auth_type_for_endpoint_format(
    transport: &GatewayProviderTransportSnapshot,
) -> String {
    let default_auth_type = transport.key.auth_type.trim().to_ascii_lowercase();
    let api_format = crate::ai_serving::normalize_api_format_alias(&transport.endpoint.api_format);
    transport
        .key
        .auth_type_by_format
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|overrides| {
            overrides
                .get(&api_format)
                .or_else(|| overrides.get(transport.endpoint.api_format.trim()))
        })
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .filter(|value| matches!(value.as_str(), "api_key" | "bearer"))
        .unwrap_or(default_auth_type)
}

fn provider_uses_bearer_like_oauth(provider_type: &str) -> bool {
    matches!(
        provider_type.trim().to_ascii_lowercase().as_str(),
        "claude_code" | "gemini_cli" | "antigravity" | "kiro"
    )
}

fn allow_auth_channel_mismatch_for_format(transport: &GatewayProviderTransportSnapshot) -> bool {
    let api_format = crate::ai_serving::normalize_api_format_alias(&transport.endpoint.api_format);
    transport
        .key
        .allow_auth_channel_mismatch_formats
        .as_ref()
        .and_then(serde_json::Value::as_array)
        .is_some_and(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .any(|item| crate::ai_serving::normalize_api_format_alias(item) == api_format)
        })
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
                event_name = "candidate_resolution_transport_load_failed",
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

#[cfg(test)]
mod tests {
    use super::candidate_auth_channel_skip_reason;
    use crate::ai_serving::GatewayProviderTransportSnapshot;
    use aether_provider_transport::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider,
    };
    use serde_json::json;

    fn sample_transport(auth_type: &str) -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "provider".to_string(),
                provider_type: "custom".to_string(),
                website: None,
                is_active: true,
                keep_priority_on_conversion: false,
                enable_format_conversion: false,
                concurrent_limit: None,
                max_retries: None,
                proxy: None,
                request_timeout_secs: None,
                stream_first_byte_timeout_secs: None,
                config: None,
            },
            endpoint: GatewayProviderTransportEndpoint {
                id: "endpoint-1".to_string(),
                provider_id: "provider-1".to_string(),
                api_format: "claude:messages".to_string(),
                api_family: Some("claude".to_string()),
                endpoint_kind: Some("messages".to_string()),
                is_active: true,
                base_url: "https://example.test".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: None,
                config: None,
                format_acceptance_config: None,
                proxy: None,
            },
            key: GatewayProviderTransportKey {
                id: "key-1".to_string(),
                provider_id: "provider-1".to_string(),
                name: "key".to_string(),
                auth_type: auth_type.to_string(),
                is_active: true,
                api_formats: Some(vec!["claude:messages".to_string()]),
                auth_type_by_format: None,
                allow_auth_channel_mismatch_formats: None,
                allowed_models: None,
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: None,
                expires_at_unix_secs: None,
                proxy: None,
                fingerprint: None,
                decrypted_api_key: "secret".to_string(),
                decrypted_auth_config: None,
            },
        }
    }

    #[test]
    fn auth_channel_gate_skips_mismatched_raw_secret_auth() {
        let transport = sample_transport("bearer");
        assert_eq!(
            candidate_auth_channel_skip_reason(&transport, Some("api_key")),
            Some("auth_channel_mismatch")
        );
    }

    #[test]
    fn auth_channel_gate_allows_explicit_mismatch_format() {
        let mut transport = sample_transport("bearer");
        transport.key.allow_auth_channel_mismatch_formats = Some(json!(["claude:messages"]));
        assert_eq!(
            candidate_auth_channel_skip_reason(&transport, Some("api_key")),
            None
        );
    }

    #[test]
    fn auth_channel_gate_treats_cli_oauth_provider_as_bearer_like() {
        let mut transport = sample_transport("oauth");
        transport.provider.provider_type = "claude_code".to_string();
        assert_eq!(
            candidate_auth_channel_skip_reason(&transport, Some("bearer_like")),
            None
        );
        assert_eq!(
            candidate_auth_channel_skip_reason(&transport, Some("api_key")),
            Some("auth_channel_mismatch")
        );
    }
}
