use std::sync::Arc;

use crate::ai_serving::planner::candidate_preparation::{
    resolve_candidate_mapped_model, resolve_candidate_oauth_auth, OauthPreparationContext,
};
use crate::ai_serving::planner::candidate_resolution::EligibleLocalExecutionCandidate;
use crate::ai_serving::planner::spec_metadata::local_same_format_provider_spec_metadata;
use crate::ai_serving::transport::kiro::KiroRequestAuth;
use crate::ai_serving::transport::vertex::resolve_local_vertex_api_key_query_auth;
use crate::ai_serving::transport::SameFormatProviderRequestBehavior;
use crate::ai_serving::{
    GatewayProviderTransportSnapshot, LocalResolvedOAuthRequestAuth, PlannerAppState,
};
use crate::AppState;

use super::super::LocalSameFormatProviderDecisionInput;
use super::super::LocalSameFormatProviderSpec;
use super::policy::{
    classify_same_format_provider_request_behavior, resolve_same_format_provider_direct_auth,
    same_format_provider_transport_supported, same_format_provider_transport_unsupported_reason,
    should_try_same_format_provider_oauth_auth,
};

pub(super) struct PreparedSameFormatProviderCandidate {
    pub(super) transport: Arc<GatewayProviderTransportSnapshot>,
    pub(super) behavior: SameFormatProviderRequestBehavior,
    pub(super) is_antigravity: bool,
    pub(super) is_claude_code: bool,
    pub(super) is_vertex: bool,
    pub(super) is_kiro: bool,
    pub(super) kiro_auth: Option<KiroRequestAuth>,
    pub(super) auth_header: Option<String>,
    pub(super) auth_value: Option<String>,
    pub(super) mapped_model: String,
    pub(super) report_kind: &'static str,
    pub(super) upstream_is_stream: bool,
}

pub(super) async fn prepare_local_same_format_provider_candidate(
    state: &AppState,
    trace_id: &str,
    input: &LocalSameFormatProviderDecisionInput,
    eligible: &EligibleLocalExecutionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    spec: LocalSameFormatProviderSpec,
) -> Option<PreparedSameFormatProviderCandidate> {
    let spec_metadata = local_same_format_provider_spec_metadata(spec);
    let planner_state = PlannerAppState::new(state);
    let candidate = &eligible.candidate;
    let transport = Arc::clone(&eligible.transport);
    let behavior = classify_same_format_provider_request_behavior(&transport, spec_metadata);

    if !same_format_provider_transport_supported(
        &behavior,
        &transport,
        spec.family,
        spec_metadata.api_format,
    ) {
        let skip_reason = same_format_provider_transport_unsupported_reason(
            &behavior,
            &transport,
            spec.family,
            spec_metadata.api_format,
        )
        .unwrap_or("transport_unsupported");
        super::super::payload::mark_skipped_local_same_format_provider_candidate(
            state,
            input,
            trace_id,
            candidate,
            candidate_index,
            candidate_id,
            skip_reason,
        )
        .await;
        return None;
    }

    let vertex_query_auth = if behavior.is_vertex {
        resolve_local_vertex_api_key_query_auth(&transport)
    } else {
        None
    };
    let should_try_oauth_auth =
        should_try_same_format_provider_oauth_auth(&behavior, &transport, spec.family);
    let oauth_auth = if should_try_oauth_auth {
        resolve_candidate_oauth_auth(
            planner_state,
            &transport,
            OauthPreparationContext {
                trace_id,
                api_format: spec_metadata.api_format,
                operation: "same_format_provider_prepare",
            },
        )
        .await
    } else {
        None
    };
    let kiro_auth = match oauth_auth.as_ref() {
        Some(LocalResolvedOAuthRequestAuth::Kiro(auth)) => Some(auth.clone()),
        _ => None,
    };
    let auth = if let Some(kiro_auth) = kiro_auth.as_ref() {
        Some((kiro_auth.name.to_string(), kiro_auth.value.clone()))
    } else if let Some(LocalResolvedOAuthRequestAuth::Header { name, value }) = oauth_auth.as_ref()
    {
        Some((name.clone(), value.clone()))
    } else {
        resolve_same_format_provider_direct_auth(&behavior, &transport, spec.family)
    };
    let (auth_header, auth_value) = match auth {
        Some((name, value)) => (Some(name), Some(value)),
        None if behavior.is_vertex && vertex_query_auth.is_some() => (None, None),
        None => {
            super::super::payload::mark_skipped_local_same_format_provider_candidate(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                "transport_auth_unavailable",
            )
            .await;
            return None;
        }
    };
    if behavior.is_vertex && vertex_query_auth.is_none() {
        super::super::payload::mark_skipped_local_same_format_provider_candidate(
            state,
            input,
            trace_id,
            candidate,
            candidate_index,
            candidate_id,
            "transport_auth_unavailable",
        )
        .await;
        return None;
    }

    let mapped_model = match resolve_candidate_mapped_model(candidate) {
        Ok(mapped_model) => mapped_model,
        Err(skip_reason) => {
            super::super::payload::mark_skipped_local_same_format_provider_candidate(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                skip_reason,
            )
            .await;
            return None;
        }
    };

    Some(PreparedSameFormatProviderCandidate {
        transport,
        behavior,
        is_antigravity: behavior.is_antigravity,
        is_claude_code: behavior.is_claude_code,
        is_vertex: behavior.is_vertex,
        is_kiro: behavior.is_kiro,
        kiro_auth,
        auth_header,
        auth_value,
        mapped_model,
        report_kind: behavior.report_kind,
        upstream_is_stream: behavior.upstream_is_stream,
    })
}
