pub mod attempt_loop;
pub mod attempt_plan;
pub mod candidate_materialization;
pub mod candidate_metadata;
pub mod candidate_persistence;
pub mod candidate_persistence_policy;
pub mod candidate_preparation;
pub mod candidate_preselection;
pub mod candidate_ranking;
pub mod candidate_resolution;
pub mod decision_input;
pub mod decision_path;
pub mod decision_payload;
pub mod dto;
pub mod execution_path;
pub mod failure_diagnostic;
pub mod plan_payload;
pub mod pool_scheduler;
pub mod ports;
pub mod ranking_metadata;
pub mod report_context;
pub mod request_body_diagnostics;
pub mod runtime_miss;
pub mod surface_spec;

pub use attempt_loop::{
    run_ai_attempt_loop, AiAttemptLoopOutcome, AiAttemptLoopPort, AiExecutionAttempt,
};
pub use attempt_plan::{
    build_ai_execution_decision_from_plan, build_ai_execution_plan_from_decision,
    extract_ai_auth_header_pair, infer_ai_upstream_base_url,
    resolve_ai_passthrough_sync_request_body, take_ai_decision_plan_core, take_ai_non_empty_string,
    take_ai_upstream_auth_pair, trim_ai_owned_non_empty_string, AiDecisionPlanCore,
    AiExecutionDecisionFromPlanParts, AiExecutionPlanFromDecisionParts, AiUpstreamAuthPair,
};
pub use candidate_materialization::{
    run_ai_candidate_materialization, AiCandidateMaterializationOutcome,
    AiCandidateMaterializationPort,
};
pub use candidate_metadata::{
    ai_local_execution_contract_for_formats, append_ai_execution_contract_fields_to_value,
    build_ai_candidate_metadata, build_ai_candidate_metadata_from_candidate,
    AiCandidateMetadataParts,
};
pub use candidate_persistence::{
    ai_candidate_extra_data_with_ranking, ai_should_persist_available_candidate_for_pool_key,
    ai_should_persist_skipped_candidate_for_pool_membership,
    run_ai_available_candidate_persistence, run_ai_skipped_candidate_persistence,
    AiAvailableCandidatePersistencePort, AiSkippedCandidatePersistencePort,
};
pub use candidate_persistence_policy::{
    ai_candidate_persistence_policy_spec, AiCandidatePersistencePolicyKind,
    AiCandidatePersistencePolicySpec,
};
pub use candidate_preparation::{
    prepare_ai_header_authenticated_candidate, resolve_ai_candidate_mapped_model,
    AiPreparedHeaderAuthenticatedCandidate,
};
pub use candidate_preselection::{
    run_ai_candidate_preselection, AiCandidatePreselectionOutcome, AiCandidatePreselectionPort,
};
pub use candidate_ranking::{
    ai_ranking_context, build_ai_rankable_candidate, run_ai_candidate_ranking,
    AiCandidateRankingPort, AiRankableCandidateParts, AiRankingContextConfig,
    AiRankingSchedulingMode,
};
pub use candidate_resolution::{
    extract_ai_pool_sticky_session_token, run_ai_candidate_resolution, AiCandidateResolutionMode,
    AiCandidateResolutionOutcome, AiCandidateResolutionPort, AiCandidateResolutionRequest,
};
pub use decision_input::{run_ai_authenticated_decision_input, AiAuthenticatedDecisionInputPort};
pub use decision_path::{
    run_ai_stream_decision_path, run_ai_sync_decision_path, AiStreamDecisionPathPort,
    AiStreamDecisionStep, AiSyncDecisionPathPort, AiSyncDecisionStep,
};
pub use decision_payload::{
    ai_execution_decision_action, build_ai_execution_decision_response,
    AiExecutionDecisionResponseParts,
};
pub use dto::{
    augment_sync_report_context, generic_decision_missing_exact_provider_request,
    AiExecutionDecision, AiExecutionPlanPayload, AiStreamAttempt, AiSyncAttempt, ConversionMode,
    ExecutionStrategy,
};
pub use execution_path::{
    run_ai_stream_execution_path, run_ai_sync_execution_path, AiPlanFallbackReason,
    AiServingExecutionOutcome, AiStreamExecutionPathPort, AiStreamExecutionStep,
    AiSyncExecutionPathPort, AiSyncExecutionStep,
};
pub use failure_diagnostic::{CandidateFailureDiagnostic, CandidateFailureDiagnosticKind};
pub use plan_payload::{
    build_ai_stream_execution_plan_payload, build_ai_sync_execution_plan_payload,
};
pub use pool_scheduler::{
    normalize_enabled_ai_pool_presets, run_ai_pool_scheduler, AiPoolCandidateFacts,
    AiPoolCandidateInput, AiPoolCandidateOrchestration, AiPoolCatalogKeyContext,
    AiPoolRuntimeState, AiPoolScheduledCandidate, AiPoolSchedulerOutcome, AiPoolSchedulingConfig,
    AiPoolSchedulingPreset, AiPoolSkippedCandidate, AI_POOL_ACCOUNT_BLOCKED_SKIP_REASON,
    AI_POOL_ACCOUNT_EXHAUSTED_SKIP_REASON, AI_POOL_COOLDOWN_SKIP_REASON,
    AI_POOL_COST_LIMIT_REACHED_SKIP_REASON,
};
pub use ranking_metadata::append_ai_ranking_metadata_to_object;
pub use report_context::{
    build_ai_execution_report_context, build_ai_report_context_original_request_echo,
    insert_provider_stream_event_api_format, provider_stream_event_api_format_for_provider_type,
    AiExecutionReportContextParts, AiRequestOrigin,
};
pub use request_body_diagnostics::{
    request_body_build_failure_extra_data, same_format_provider_request_body_failure_extra_data,
};
pub use runtime_miss::{
    apply_ai_runtime_candidate_evaluation_progress,
    apply_ai_runtime_candidate_evaluation_progress_preserving_candidate_signal,
    apply_ai_runtime_candidate_evaluation_progress_to_diagnostic,
    apply_ai_runtime_candidate_terminal_plan_reason_to_diagnostic,
    apply_ai_runtime_candidate_terminal_reason, build_ai_runtime_candidate_evaluation_diagnostic,
    build_ai_runtime_execution_exhausted_diagnostic, record_ai_runtime_candidate_skip_reason,
    record_ai_runtime_candidate_skip_reason_on_diagnostic,
    set_ai_runtime_candidate_evaluation_diagnostic, set_ai_runtime_execution_exhausted_diagnostic,
    set_ai_runtime_miss_diagnostic_reason, AiRuntimeMissDiagnosticFields,
    AiRuntimeMissDiagnosticPort,
};
pub use surface_spec::{
    ai_gemini_files_spec_metadata, ai_openai_image_spec_metadata,
    ai_openai_responses_spec_metadata, ai_requested_model_family_for_same_format_provider,
    ai_requested_model_family_for_standard_source, ai_requested_model_family_for_video_create,
    ai_same_format_provider_spec_metadata, ai_standard_spec_metadata,
    ai_video_create_spec_metadata, extract_ai_gemini_model_from_path,
    extract_ai_requested_model_from_request_path, extract_ai_standard_requested_model,
    AiExecutionSurfaceSpecMetadata, AiRequestedModelFamily,
};
