use crate::ai_pipeline::contracts::{
    GatewayControlPlanResponse, GatewayControlSyncDecisionResponse,
};
use crate::ai_pipeline::GatewayControlDecision;
use crate::{AppState, GatewayError};

mod candidate_affinity;
mod candidate_eligibility;
mod candidate_materialization;
mod candidate_metadata;
mod candidate_preparation;
mod candidate_source;
mod common;
mod decision;
mod decision_input;
mod materialization_policy;
mod passthrough;
mod payload_metadata;
mod plan_builders;
mod pool_scheduler;
mod report_context;
mod route;
mod runtime_miss;
mod spec_metadata;
mod specialized;
mod standard;
mod state;

pub(crate) use self::candidate_eligibility::extract_pool_sticky_session_token;
pub(crate) use self::passthrough::{
    build_local_same_format_stream_plan_and_reports, build_local_same_format_sync_plan_and_reports,
};
pub(crate) use self::plan_builders::{
    build_gemini_stream_plan_from_decision, build_gemini_sync_plan_from_decision,
    build_openai_cli_stream_plan_from_decision, build_openai_cli_sync_plan_from_decision,
    build_passthrough_sync_plan_from_decision, build_standard_stream_plan_from_decision,
    build_standard_sync_plan_from_decision, LocalStreamPlanAndReport, LocalSyncPlanAndReport,
};
pub(crate) use self::specialized::{
    build_local_gemini_files_stream_plan_and_reports_for_kind,
    build_local_gemini_files_sync_plan_and_reports_for_kind,
    build_local_image_sync_plan_and_reports_for_kind,
    build_local_video_sync_plan_and_reports_for_kind,
};
pub(crate) use self::standard::{
    build_local_openai_chat_stream_plan_and_reports_for_kind,
    build_local_openai_chat_sync_plan_and_reports_for_kind,
    build_local_openai_cli_stream_plan_and_reports_for_kind,
    build_local_openai_cli_sync_plan_and_reports_for_kind,
    build_local_stream_plan_and_reports as build_standard_family_stream_plan_and_reports,
    build_local_sync_plan_and_reports as build_standard_family_sync_plan_and_reports,
    set_local_openai_chat_execution_exhausted_diagnostic,
};
pub(crate) use self::state::{
    GatewayAuthApiKeySnapshot, GatewayProviderTransportSnapshot, LocalResolvedOAuthRequestAuth,
    PlannerAppState,
};

pub(crate) async fn maybe_build_sync_decision_payload(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    body_base64: Option<&str>,
    body_is_empty: bool,
) -> Result<Option<GatewayControlSyncDecisionResponse>, GatewayError> {
    decision::maybe_build_sync_decision_payload(
        state,
        parts,
        trace_id,
        decision,
        body_json,
        body_base64,
        body_is_empty,
    )
    .await
}

pub(crate) async fn maybe_build_stream_decision_payload(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
) -> Result<Option<GatewayControlSyncDecisionResponse>, GatewayError> {
    decision::maybe_build_stream_decision_payload(state, parts, trace_id, decision, body_json).await
}

pub(crate) async fn maybe_build_sync_plan_payload(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    body_base64: Option<&str>,
    body_is_empty: bool,
) -> Result<Option<GatewayControlPlanResponse>, GatewayError> {
    decision::maybe_build_sync_plan_payload_impl(
        state,
        parts,
        trace_id,
        decision,
        body_json,
        body_base64,
        body_is_empty,
    )
    .await
}

pub(crate) async fn maybe_build_stream_plan_payload(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
) -> Result<Option<GatewayControlPlanResponse>, GatewayError> {
    decision::maybe_build_stream_plan_payload_impl(state, parts, trace_id, decision, body_json)
        .await
}
