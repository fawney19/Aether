mod affinity;
mod auth;
mod candidate;
mod health;
mod model;
mod provider;
mod ranking;
mod request_candidate;

pub use affinity::{
    build_scheduler_affinity_cache_key_for_api_key_id, candidate_affinity_hash, candidate_key,
    matches_affinity_target, SchedulerAffinityTarget,
};
pub use auth::{
    api_format_matches_allowed_value, auth_constraints_allow_api_format,
    auth_constraints_allow_model, auth_constraints_allow_provider, provider_matches_allowed_value,
    SchedulerAuthConstraints,
};
pub use candidate::{
    auth_api_key_concurrency_limit_reached, candidate_is_selectable_with_runtime_state,
    candidate_runtime_skip_reason_with_state, candidate_supports_required_capability,
    collect_global_model_names_for_required_capability, enumerate_minimal_candidate_selection,
    requested_capability_priority_for_candidate, CandidateRuntimeSelectabilityInput,
    EnumerateMinimalCandidateSelectionInput, SchedulerMinimalCandidateSelectionCandidate,
    SchedulerPriorityMode,
};
pub use health::{
    aggregate_provider_key_health_score, count_recent_active_requests_for_api_key,
    count_recent_active_requests_for_provider, count_recent_active_requests_for_provider_key,
    count_recent_rpm_requests_for_provider_key, count_recent_rpm_requests_for_provider_key_since,
    effective_provider_key_health_score, effective_provider_key_rpm_limit,
    is_candidate_in_recent_failure_cooldown, is_provider_key_circuit_open,
    provider_key_health_bucket, provider_key_health_score, provider_key_rpm_allows_request,
    provider_key_rpm_allows_request_since, ProviderKeyHealthBucket, PROVIDER_KEY_RPM_WINDOW_SECS,
};
pub use model::{
    candidate_model_names, extract_global_priority_for_format, matches_model_mapping,
    normalize_api_format, resolve_provider_model_name, resolve_requested_global_model_name,
    row_supports_requested_model, row_supports_required_capability, select_provider_model_name,
};
pub use provider::{build_provider_concurrent_limit_map, should_skip_provider_quota};
pub use ranking::{
    apply_scheduler_candidate_ranking, SchedulerRankableCandidate, SchedulerRankingContext,
    SchedulerRankingMode, SchedulerRankingOutcome, SchedulerTunnelAffinityBucket,
    RANKING_REASON_CACHED_AFFINITY, RANKING_REASON_CROSS_FORMAT, RANKING_REASON_LOCAL_TUNNEL,
};
pub use request_candidate::{
    build_execution_request_candidate_seed, build_local_request_candidate_status_record,
    build_report_request_candidate_status_record, execution_error_details,
    finalize_execution_request_candidate_report_context, is_terminal_candidate_status,
    parse_request_candidate_report_context, resolve_report_request_candidate_slot,
    LocalRequestCandidateStatusRecordInput, ReportRequestCandidateStatusRecordInput,
    SchedulerExecutionRequestCandidateSeed, SchedulerRequestCandidateReportContext,
    SchedulerRequestCandidateStatusUpdate, SchedulerResolvedReportRequestCandidateSlot,
};
