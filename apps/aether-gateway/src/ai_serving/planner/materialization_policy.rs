use aether_ai_serving::ai_candidate_persistence_policy_spec;
pub(crate) use aether_ai_serving::AiCandidatePersistencePolicyKind as LocalCandidatePersistencePolicyKind;
use serde_json::Value;

use crate::ai_serving::planner::candidate_materialization::{
    LocalAvailableCandidatePersistenceContext, LocalSkippedCandidatePersistenceContext,
};
use crate::ai_serving::ExecutionRuntimeAuthContext;

#[derive(Debug, Clone, Copy)]
pub(crate) struct LocalCandidatePersistencePolicy<'a> {
    pub(crate) available: LocalAvailableCandidatePersistenceContext<'a>,
    pub(crate) skipped: LocalSkippedCandidatePersistenceContext<'a>,
}

pub(crate) fn build_local_candidate_persistence_policy<'a>(
    auth_context: &'a ExecutionRuntimeAuthContext,
    required_capabilities: Option<&'a Value>,
    kind: LocalCandidatePersistencePolicyKind,
) -> LocalCandidatePersistencePolicy<'a> {
    let spec = ai_candidate_persistence_policy_spec(kind);

    LocalCandidatePersistencePolicy {
        available: LocalAvailableCandidatePersistenceContext {
            user_id: &auth_context.user_id,
            api_key_id: &auth_context.api_key_id,
            required_capabilities,
            error_context: spec.available_error_context,
        },
        skipped: LocalSkippedCandidatePersistenceContext {
            user_id: &auth_context.user_id,
            api_key_id: &auth_context.api_key_id,
            required_capabilities,
            error_context: spec.skipped_error_context,
            record_runtime_miss_diagnostic: spec.record_runtime_miss_diagnostic,
        },
    }
}
