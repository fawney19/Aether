use crate::ai_pipeline::contracts::ExecutionRuntimeAuthContext;
use crate::ai_pipeline::GatewayAuthApiKeySnapshot;

mod build;
mod candidates;
mod payload;

pub(crate) use self::build::{
    build_local_stream_plan_and_reports, build_local_sync_plan_and_reports,
    maybe_build_stream_via_standard_family_payload, maybe_build_sync_via_standard_family_payload,
};
pub(crate) use crate::ai_pipeline::{
    LocalStandardSourceFamily, LocalStandardSourceMode, LocalStandardSpec,
};

#[derive(Debug, Clone)]
pub(super) struct LocalStandardDecisionInput {
    pub(super) auth_context: ExecutionRuntimeAuthContext,
    pub(super) requested_model: String,
    pub(super) auth_snapshot: GatewayAuthApiKeySnapshot,
}

#[derive(Debug, Clone)]
pub(super) struct LocalStandardCandidateAttempt {
    pub(super) candidate: aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate,
    pub(super) candidate_index: u32,
    pub(super) candidate_id: String,
}
