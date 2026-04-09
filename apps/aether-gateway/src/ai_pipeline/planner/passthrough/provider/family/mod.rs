use crate::ai_pipeline::contracts::ExecutionRuntimeAuthContext;
use crate::ai_pipeline::GatewayAuthApiKeySnapshot;

mod build;
mod candidates;
mod payload;

pub(crate) use self::build::{
    maybe_build_stream_local_same_format_provider_decision_payload,
    maybe_build_sync_local_same_format_provider_decision_payload,
};
pub(crate) use self::candidates::{
    materialize_local_same_format_provider_candidate_attempts,
    resolve_local_same_format_provider_decision_input,
};
pub(crate) use self::payload::maybe_build_local_same_format_provider_decision_payload_for_candidate;
pub(crate) use crate::ai_pipeline::{LocalSameFormatProviderFamily, LocalSameFormatProviderSpec};

#[derive(Debug, Clone)]
pub(crate) struct LocalSameFormatProviderDecisionInput {
    pub(crate) auth_context: ExecutionRuntimeAuthContext,
    pub(crate) requested_model: String,
    pub(crate) auth_snapshot: GatewayAuthApiKeySnapshot,
}

#[derive(Debug, Clone)]
pub(crate) struct LocalSameFormatProviderCandidateAttempt {
    pub(crate) candidate: aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate,
    pub(crate) candidate_index: u32,
    pub(crate) candidate_id: String,
}
