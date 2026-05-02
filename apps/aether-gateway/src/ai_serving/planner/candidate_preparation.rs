use aether_ai_serving::{
    prepare_ai_header_authenticated_candidate, resolve_ai_candidate_mapped_model,
    AiPreparedHeaderAuthenticatedCandidate,
};
use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;
use tracing::warn;

use crate::ai_serving::{
    GatewayProviderTransportSnapshot, LocalResolvedOAuthRequestAuth, PlannerAppState,
};

pub(crate) type PreparedHeaderAuthenticatedCandidate = AiPreparedHeaderAuthenticatedCandidate;

#[derive(Debug, Clone, Copy)]
pub(crate) struct OauthPreparationContext<'a> {
    pub(crate) trace_id: &'a str,
    pub(crate) api_format: &'a str,
    pub(crate) operation: &'a str,
}

pub(crate) async fn prepare_header_authenticated_candidate(
    state: PlannerAppState<'_>,
    transport: &GatewayProviderTransportSnapshot,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    direct_auth: Option<(String, String)>,
    context: OauthPreparationContext<'_>,
) -> Result<PreparedHeaderAuthenticatedCandidate, &'static str> {
    let oauth_auth = if direct_auth.is_none() {
        match resolve_candidate_oauth_auth(state, transport, context).await {
            Some(LocalResolvedOAuthRequestAuth::Header { name, value }) => Some((name, value)),
            Some(LocalResolvedOAuthRequestAuth::Kiro(_)) => None,
            None => None,
        }
    } else {
        None
    };

    prepare_ai_header_authenticated_candidate(
        direct_auth,
        oauth_auth,
        candidate.selected_provider_model_name.as_str(),
    )
}

pub(crate) fn prepare_header_authenticated_candidate_from_auth(
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    auth_header: String,
    auth_value: String,
) -> Result<PreparedHeaderAuthenticatedCandidate, &'static str> {
    prepare_ai_header_authenticated_candidate(
        Some((auth_header, auth_value)),
        None,
        candidate.selected_provider_model_name.as_str(),
    )
}

pub(crate) fn resolve_candidate_mapped_model(
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
) -> Result<String, &'static str> {
    resolve_ai_candidate_mapped_model(candidate.selected_provider_model_name.as_str())
}

pub(crate) async fn resolve_candidate_oauth_auth(
    state: PlannerAppState<'_>,
    transport: &GatewayProviderTransportSnapshot,
    context: OauthPreparationContext<'_>,
) -> Option<LocalResolvedOAuthRequestAuth> {
    match state.resolve_local_oauth_request_auth(transport).await {
        Ok(Some(auth)) => Some(auth),
        Ok(None) => None,
        Err(err) => {
            warn!(
                event_name = "candidate_preparation_oauth_auth_resolution_failed",
                log_type = "event",
                trace_id = %context.trace_id,
                api_format = %context.api_format,
                operation = %context.operation,
                provider_type = %transport.provider.provider_type,
                error = ?err,
                "failed to resolve oauth auth while preparing local candidate"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use aether_provider_transport::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };
    use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;

    use super::{prepare_header_authenticated_candidate, OauthPreparationContext};
    use crate::ai_serving::PlannerAppState;

    fn sample_transport() -> GatewayProviderTransportSnapshot {
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
                api_format: "openai:chat".to_string(),
                api_family: Some("openai".to_string()),
                endpoint_kind: Some("chat".to_string()),
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
                auth_type: "api_key".to_string(),
                is_active: true,
                api_formats: Some(vec!["openai:chat".to_string()]),
                auth_type_by_format: None,
                allow_auth_channel_mismatch_formats: None,
                allowed_models: None,
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: None,
                expires_at_unix_secs: None,
                proxy: None,
                fingerprint: None,
                decrypted_api_key: String::new(),
                decrypted_auth_config: None,
            },
        }
    }

    fn sample_candidate() -> SchedulerMinimalCandidateSelectionCandidate {
        SchedulerMinimalCandidateSelectionCandidate {
            provider_id: "provider-1".to_string(),
            provider_name: "provider".to_string(),
            provider_type: "custom".to_string(),
            provider_priority: 1,
            endpoint_id: "endpoint-1".to_string(),
            endpoint_api_format: "openai:chat".to_string(),
            key_id: "key-1".to_string(),
            key_name: "key".to_string(),
            key_auth_type: "api_key".to_string(),
            key_internal_priority: 1,
            key_global_priority_for_format: None,
            key_capabilities: None,
            model_id: "model-1".to_string(),
            global_model_id: "global-model-1".to_string(),
            global_model_name: "gpt-test".to_string(),
            selected_provider_model_name: "gpt-test-upstream".to_string(),
            mapping_matched_model: None,
        }
    }

    #[tokio::test]
    async fn header_auth_preparation_allows_empty_auth_value() {
        let state = crate::AppState::new().expect("state should build");
        let transport = sample_transport();
        let candidate = sample_candidate();

        let prepared = prepare_header_authenticated_candidate(
            PlannerAppState::new(&state),
            &transport,
            &candidate,
            Some(("authorization".to_string(), String::new())),
            OauthPreparationContext {
                trace_id: "trace-empty-auth",
                api_format: "openai:chat",
                operation: "test",
            },
        )
        .await
        .expect("empty auth value should still prepare the candidate");

        assert_eq!(prepared.auth_header, "authorization");
        assert_eq!(prepared.auth_value, "");
        assert_eq!(prepared.mapped_model, "gpt-test-upstream");
    }
}
