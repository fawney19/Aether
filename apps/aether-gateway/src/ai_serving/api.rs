use crate::ai_serving::{is_json_request, GatewayControlDecision};

pub(crate) use crate::ai_serving::{
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
    maybe_build_stream_decision_payload, maybe_build_stream_plan_payload,
    maybe_build_sync_decision_payload, maybe_build_sync_plan_payload,
    set_local_openai_chat_execution_exhausted_diagnostic,
};
pub(crate) use crate::ai_serving::{
    maybe_bridge_standard_sync_json_to_stream, maybe_build_provider_private_stream_normalizer,
    maybe_build_stream_response_rewriter, maybe_build_sync_finalize_outcome,
    maybe_compile_sync_finalize_response, LocalCoreSyncFinalizeOutcome,
};
pub(crate) use crate::ai_serving::{
    AiExecutionDecision, AiExecutionPlanPayload, AiStreamAttempt, AiSyncAttempt,
};
pub(crate) use aether_ai_formats::api::{
    build_core_error_body_for_client_format, core_error_background_report_kind,
    core_error_default_client_api_format, core_success_background_report_kind,
    encode_kiro_sse_events, implicit_sync_finalize_report_kind, is_core_error_finalize_kind,
    normalize_provider_private_report_context, normalize_provider_private_response_value,
    provider_private_response_allows_sync_finalize, resolve_claude_stream_spec,
    resolve_claude_sync_spec, resolve_gemini_stream_spec, resolve_gemini_sync_spec,
    resolve_local_image_stream_spec, resolve_local_image_sync_spec,
    resolve_local_same_format_stream_spec, resolve_local_same_format_sync_spec,
    AiControlPlanRequest, ExecutionRuntimeAuthContext, LocalCoreSyncErrorKind,
    LocalOpenAiImageSpec, LocalSameFormatProviderFamily, LocalSameFormatProviderSpec,
    LocalStandardSourceFamily, LocalStandardSourceMode, LocalStandardSpec,
    StreamingStandardTerminalObserver, EXECUTION_RUNTIME_STREAM_DECISION_ACTION,
    EXECUTION_RUNTIME_SYNC_DECISION_ACTION, GEMINI_FILES_DOWNLOAD_PLAN_KIND,
    GEMINI_VIDEO_CANCEL_SYNC_PLAN_KIND, OPENAI_IMAGE_STREAM_PLAN_KIND,
    OPENAI_IMAGE_SYNC_FINALIZE_REPORT_KIND, OPENAI_IMAGE_SYNC_PLAN_KIND,
    OPENAI_VIDEO_CANCEL_SYNC_PLAN_KIND, OPENAI_VIDEO_CONTENT_PLAN_KIND,
    OPENAI_VIDEO_DELETE_SYNC_PLAN_KIND, OPENAI_VIDEO_REMIX_SYNC_PLAN_KIND,
};

pub(crate) fn parse_direct_request_body(
    parts: &http::request::Parts,
    body_bytes: &axum::body::Bytes,
) -> Option<(serde_json::Value, Option<String>)> {
    aether_ai_formats::api::parse_direct_request_body(
        is_json_request(&parts.headers),
        body_bytes.as_ref(),
    )
}

pub(crate) fn resolve_execution_runtime_stream_plan_kind(
    parts: &http::request::Parts,
    decision: &GatewayControlDecision,
) -> Option<&'static str> {
    aether_ai_formats::api::resolve_execution_runtime_stream_plan_kind(
        decision.route_class.as_deref(),
        decision.route_family.as_deref(),
        decision.route_kind.as_deref(),
        decision.request_auth_channel.as_deref(),
        &parts.method,
        parts.uri.path(),
    )
}

pub(crate) fn resolve_execution_runtime_sync_plan_kind(
    parts: &http::request::Parts,
    decision: &GatewayControlDecision,
) -> Option<&'static str> {
    aether_ai_formats::api::resolve_execution_runtime_sync_plan_kind(
        decision.route_class.as_deref(),
        decision.route_family.as_deref(),
        decision.route_kind.as_deref(),
        decision.request_auth_channel.as_deref(),
        &parts.method,
        parts.uri.path(),
    )
}

pub(crate) fn is_matching_stream_request(
    plan_kind: &str,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    body_base64: Option<&str>,
) -> bool {
    crate::ai_serving::planner_is_matching_stream_request(plan_kind, parts, body_json, body_base64)
}

pub(crate) fn supports_sync_execution_decision_kind(plan_kind: &str) -> bool {
    aether_ai_formats::api::supports_sync_execution_decision_kind(plan_kind)
}

pub(crate) fn supports_stream_execution_decision_kind(plan_kind: &str) -> bool {
    aether_ai_formats::api::supports_stream_execution_decision_kind(plan_kind)
}

pub(crate) fn aggregate_openai_chat_stream_sync_response(body: &[u8]) -> Option<serde_json::Value> {
    aether_ai_formats::api::aggregate_openai_chat_stream_sync_response(body)
}

pub(crate) fn aggregate_openai_responses_stream_sync_response(
    body: &[u8],
) -> Option<serde_json::Value> {
    aether_ai_formats::api::aggregate_openai_responses_stream_sync_response(body)
}

pub(crate) fn aggregate_claude_stream_sync_response(body: &[u8]) -> Option<serde_json::Value> {
    aether_ai_formats::api::aggregate_claude_stream_sync_response(body)
}

pub(crate) fn aggregate_gemini_stream_sync_response(body: &[u8]) -> Option<serde_json::Value> {
    aether_ai_formats::api::aggregate_gemini_stream_sync_response(body)
}
