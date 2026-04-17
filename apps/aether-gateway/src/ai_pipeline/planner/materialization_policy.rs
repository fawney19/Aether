use serde_json::Value;

use crate::ai_pipeline::contracts::ExecutionRuntimeAuthContext;
use crate::ai_pipeline::planner::candidate_materialization::{
    LocalAvailableCandidatePersistenceContext, LocalSkippedCandidatePersistenceContext,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub(crate) enum LocalCandidatePersistencePolicyKind {
    StandardDecision,
    SameFormatProviderDecision,
    OpenAiChatDecision,
    OpenAiCliDecision,
    GeminiFilesDecision,
    VideoDecision,
}

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
    let (available_error_context, skipped_error_context, record_runtime_miss_diagnostic) =
        match kind {
            LocalCandidatePersistencePolicyKind::StandardDecision => (
                "gateway local standard decision request candidate upsert failed",
                "gateway local standard decision failed to persist skipped candidate",
                true,
            ),
            LocalCandidatePersistencePolicyKind::SameFormatProviderDecision => (
                "gateway local same-format decision request candidate upsert failed",
                "gateway local same-format decision failed to persist skipped candidate",
                true,
            ),
            LocalCandidatePersistencePolicyKind::OpenAiChatDecision => (
                "gateway local openai chat decision request candidate upsert failed",
                "gateway local openai chat decision failed to persist skipped candidate",
                true,
            ),
            LocalCandidatePersistencePolicyKind::OpenAiCliDecision => (
                "gateway local openai cli decision request candidate upsert failed",
                "gateway local openai cli decision failed to persist skipped candidate",
                true,
            ),
            LocalCandidatePersistencePolicyKind::GeminiFilesDecision => (
                "gateway local gemini files request candidate upsert failed",
                "gateway local gemini files failed to persist skipped candidate",
                false,
            ),
            LocalCandidatePersistencePolicyKind::VideoDecision => (
                "gateway local video decision request candidate upsert failed",
                "gateway local video decision failed to persist skipped candidate",
                false,
            ),
        };

    LocalCandidatePersistencePolicy {
        available: LocalAvailableCandidatePersistenceContext {
            user_id: &auth_context.user_id,
            api_key_id: &auth_context.api_key_id,
            required_capabilities,
            error_context: available_error_context,
        },
        skipped: LocalSkippedCandidatePersistenceContext {
            user_id: &auth_context.user_id,
            api_key_id: &auth_context.api_key_id,
            required_capabilities,
            error_context: skipped_error_context,
            record_runtime_miss_diagnostic,
        },
    }
}
