mod adaptation;
mod contracts;
mod conversion;
mod finalize;
mod planner;
mod pure;
pub(crate) mod transport;

use axum::body::Body;
use axum::http::{Response, Uri};

use crate::{usage::GatewaySyncReportRequest, AppState, GatewayError};

use self::contracts::ExecutionRuntimeAuthContext;

pub(crate) use self::adaptation::maybe_build_provider_private_stream_normalizer;
pub(crate) use self::finalize::common::LocalCoreSyncFinalizeOutcome;
pub(crate) use self::finalize::internal::{
    maybe_build_stream_response_rewriter, maybe_build_sync_finalize_outcome,
    maybe_compile_sync_finalize_response,
};
pub(crate) use self::planner::{
    build_gemini_stream_plan_from_decision, build_gemini_sync_plan_from_decision,
    build_local_gemini_files_stream_plan_and_reports_for_kind,
    build_local_gemini_files_sync_plan_and_reports_for_kind,
    build_local_openai_chat_stream_plan_and_reports_for_kind,
    build_local_openai_chat_sync_plan_and_reports_for_kind,
    build_local_openai_cli_stream_plan_and_reports_for_kind,
    build_local_openai_cli_sync_plan_and_reports_for_kind,
    build_local_same_format_stream_plan_and_reports, build_local_same_format_sync_plan_and_reports,
    build_local_video_sync_plan_and_reports_for_kind, build_openai_cli_stream_plan_from_decision,
    build_openai_cli_sync_plan_from_decision, build_passthrough_sync_plan_from_decision,
    build_standard_family_stream_plan_and_reports, build_standard_family_sync_plan_and_reports,
    build_standard_stream_plan_from_decision, build_standard_sync_plan_from_decision,
    maybe_build_stream_decision_payload, maybe_build_stream_plan_payload,
    maybe_build_sync_decision_payload, maybe_build_sync_plan_payload,
    set_local_openai_chat_execution_exhausted_diagnostic, GatewayAuthApiKeySnapshot,
    GatewayProviderTransportSnapshot, LocalResolvedOAuthRequestAuth, PlannerAppState,
};
pub(crate) use self::pure::*;
pub(crate) use crate::control::GatewayControlDecision;
pub(crate) use crate::execution_runtime::{ConversionMode, ExecutionStrategy};

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

pub(crate) fn is_json_request(headers: &http::HeaderMap) -> bool {
    crate::headers::is_json_request(headers)
}

pub(crate) fn build_execution_runtime_auth_context(
    auth_context: &crate::control::GatewayControlAuthContext,
) -> ExecutionRuntimeAuthContext {
    ExecutionRuntimeAuthContext {
        user_id: auth_context.user_id.clone(),
        api_key_id: auth_context.api_key_id.clone(),
        balance_remaining: auth_context.balance_remaining,
        access_allowed: auth_context.access_allowed,
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

pub(crate) fn maybe_build_local_sync_finalize_response(
    trace_id: &str,
    decision: &GatewayControlDecision,
    payload: &GatewaySyncReportRequest,
) -> Result<Option<Response<Body>>, GatewayError> {
    crate::execution_runtime::maybe_build_local_sync_finalize_response(trace_id, decision, payload)
}
