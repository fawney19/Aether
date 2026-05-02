mod adaptation;
pub(crate) mod api;
mod finalize;
mod planner;
mod pure;
pub(crate) mod transport;

use axum::body::Body;
use axum::http::{Response, Uri};

use crate::{usage::GatewaySyncReportRequest, AppState, GatewayError};

pub(crate) use self::adaptation::{
    maybe_build_provider_private_stream_normalizer, ProviderPrivateStreamNormalizer,
};
pub(crate) use self::finalize::common::LocalCoreSyncFinalizeOutcome;
pub(crate) use self::finalize::internal::{
    maybe_bridge_standard_sync_json_to_stream, maybe_build_stream_response_rewriter,
    maybe_build_sync_finalize_outcome, maybe_compile_sync_finalize_response,
    SyncToStreamBridgeOutcome,
};
pub(crate) use self::planner::{
    build_gemini_stream_plan_from_decision, build_gemini_sync_plan_from_decision,
    build_local_gemini_files_stream_plan_and_reports_for_kind,
    build_local_gemini_files_sync_plan_and_reports_for_kind,
    build_local_image_stream_plan_and_reports_for_kind,
    build_local_image_sync_plan_and_reports_for_kind,
    build_local_openai_chat_stream_plan_and_reports_for_kind,
    build_local_openai_chat_sync_plan_and_reports_for_kind,
    build_local_openai_responses_stream_plan_and_reports_for_kind,
    build_local_openai_responses_sync_plan_and_reports_for_kind,
    build_local_same_format_stream_plan_and_reports, build_local_same_format_sync_plan_and_reports,
    build_local_video_sync_plan_and_reports_for_kind,
    build_openai_responses_stream_plan_from_decision,
    build_openai_responses_sync_plan_from_decision, build_passthrough_sync_plan_from_decision,
    build_standard_family_stream_plan_and_reports, build_standard_family_sync_plan_and_reports,
    build_standard_stream_plan_from_decision, build_standard_sync_plan_from_decision,
    extract_pool_sticky_session_token, maybe_build_stream_decision_payload,
    maybe_build_stream_plan_payload, maybe_build_sync_decision_payload,
    maybe_build_sync_plan_payload, planner_is_matching_stream_request,
    set_local_openai_chat_execution_exhausted_diagnostic, CandidateFailureDiagnostic,
    CandidateFailureDiagnosticKind, GatewayAuthApiKeySnapshot, GatewayProviderTransportSnapshot,
    LocalResolvedOAuthRequestAuth, PlannerAppState,
};
pub(crate) use self::pure::*;
pub(crate) use self::transport::{
    append_transport_diagnostics_to_value, build_request_trace_proxy_value,
    candidate_common_transport_skip_reason, candidate_transport_pair_skip_reason,
    request_conversion_direct_auth, request_conversion_enabled_for_transport,
    request_conversion_transport_supported, request_conversion_transport_unsupported_reason,
    request_pair_allowed_for_transport, CandidateTransportPolicyFacts,
};
pub(crate) use crate::control::GatewayControlDecision;
pub(crate) use crate::execution_runtime::{ConversionMode, ExecutionStrategy};
pub(crate) use crate::headers::RequestOrigin;
pub(crate) use aether_ai_serving::{
    ai_local_execution_contract_for_formats, augment_sync_report_context,
    build_ai_report_context_original_request_echo as build_report_context_original_request_echo,
    extract_ai_gemini_model_from_path as extract_gemini_model_from_path,
    generic_decision_missing_exact_provider_request as generic_decision_missing_exact_provider_request_impl,
    AiExecutionDecision, AiExecutionPlanPayload, AiStreamAttempt, AiSyncAttempt,
};

pub(crate) fn build_provider_transport_request_url(
    transport: &GatewayProviderTransportSnapshot,
    provider_api_format: &str,
    mapped_model: Option<&str>,
    upstream_is_stream: bool,
    request_query: Option<&str>,
    kiro_api_region: Option<&str>,
) -> Option<String> {
    self::transport::build_transport_request_url(
        transport,
        self::transport::TransportRequestUrlParams {
            provider_api_format,
            mapped_model,
            upstream_is_stream,
            request_query,
            kiro_api_region,
        },
    )
}

pub(crate) async fn resolve_execution_runtime_auth_context(
    state: &AppState,
    decision: &GatewayControlDecision,
    headers: &http::HeaderMap,
    uri: &Uri,
    trace_id: &str,
) -> Result<Option<crate::control::GatewayControlAuthContext>, GatewayError> {
    crate::control::resolve_execution_runtime_auth_context(state, decision, headers, uri, trace_id)
        .await
}

pub(crate) fn collect_control_headers(
    headers: &http::HeaderMap,
) -> std::collections::BTreeMap<String, String> {
    crate::headers::collect_control_headers(headers)
}

pub(crate) fn request_origin_from_headers(headers: &http::HeaderMap) -> RequestOrigin {
    crate::headers::request_origin_from_headers(headers)
}

pub(crate) fn request_origin_from_parts(parts: &http::request::Parts) -> RequestOrigin {
    crate::headers::request_origin_from_parts(parts)
}

pub(crate) fn is_json_request(headers: &http::HeaderMap) -> bool {
    crate::headers::is_json_request(headers)
}

pub(crate) fn build_execution_runtime_auth_context(
    auth_context: &crate::control::GatewayControlAuthContext,
) -> ExecutionRuntimeAuthContext {
    ExecutionRuntimeAuthContext {
        user_id: auth_context.user_id.clone(),
        api_key_id: auth_context.api_key_id.clone(),
        username: auth_context.username.clone(),
        api_key_name: auth_context.api_key_name.clone(),
        balance_remaining: auth_context.balance_remaining,
        access_allowed: auth_context.access_allowed,
        api_key_is_standalone: auth_context.api_key_is_standalone,
    }
}

pub(crate) fn resolve_decision_execution_runtime_auth_context(
    decision: &GatewayControlDecision,
) -> Option<ExecutionRuntimeAuthContext> {
    decision
        .auth_context
        .as_ref()
        .map(build_execution_runtime_auth_context)
}

pub(crate) fn resolve_local_decision_execution_runtime_auth_context(
    decision: &GatewayControlDecision,
) -> Option<ExecutionRuntimeAuthContext> {
    resolve_decision_execution_runtime_auth_context(decision).filter(|auth_context| {
        !auth_context.user_id.trim().is_empty() && !auth_context.api_key_id.trim().is_empty()
    })
}

pub(crate) fn generic_decision_missing_exact_provider_request(
    payload: &AiExecutionDecision,
) -> bool {
    if !generic_decision_missing_exact_provider_request_impl(payload) {
        return false;
    }

    tracing::warn!(
        decision_kind = payload.decision_kind.as_deref().unwrap_or_default(),
        provider_api_format = payload.provider_api_format.as_deref().unwrap_or_default(),
        client_api_format = payload.client_api_format.as_deref().unwrap_or_default(),
        "gateway generic decision missing exact provider request; falling back to plan"
    );
    true
}

pub(crate) fn maybe_build_local_sync_finalize_response(
    trace_id: &str,
    decision: &GatewayControlDecision,
    payload: &GatewaySyncReportRequest,
) -> Result<Option<Response<Body>>, GatewayError> {
    crate::execution_runtime::maybe_build_local_sync_finalize_response(trace_id, decision, payload)
}
