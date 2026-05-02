use std::collections::BTreeMap;

pub(crate) use aether_ai_serving::{
    build_ai_execution_plan_from_decision, resolve_ai_passthrough_sync_request_body,
    take_ai_decision_plan_core, take_ai_non_empty_string as take_non_empty_string,
    take_ai_upstream_auth_pair, AiExecutionPlanFromDecisionParts,
};

use crate::ai_serving::augment_sync_report_context as augment_sync_report_context_impl;
pub(crate) use crate::ai_serving::{
    generic_decision_missing_exact_provider_request, AiStreamAttempt, AiSyncAttempt,
};
use crate::{AiExecutionDecision, GatewayError};

#[path = "standard/gemini/plan_builders.rs"]
mod gemini_builders;
#[path = "standard/openai/plan_builders.rs"]
mod openai_builders;
#[path = "passthrough/plan_builders.rs"]
mod passthrough_builders;
#[path = "standard/plan_builders.rs"]
mod standard_builders;

pub(crate) use gemini_builders::{
    build_gemini_stream_plan_from_decision, build_gemini_sync_plan_from_decision,
};
pub(crate) use openai_builders::{
    build_openai_chat_stream_plan_from_decision, build_openai_chat_sync_plan_from_decision,
    build_openai_responses_stream_plan_from_decision,
    build_openai_responses_sync_plan_from_decision,
};
pub(crate) use passthrough_builders::{
    build_passthrough_stream_plan_from_decision, build_passthrough_sync_plan_from_decision,
};
pub(crate) use standard_builders::{
    build_standard_stream_plan_from_decision, build_standard_sync_plan_from_decision,
};

pub(super) fn augment_sync_report_context(
    report_context: Option<serde_json::Value>,
    provider_request_headers: &BTreeMap<String, String>,
    provider_request_body: &serde_json::Value,
) -> Result<Option<serde_json::Value>, GatewayError> {
    augment_sync_report_context_impl(
        report_context,
        provider_request_headers,
        provider_request_body,
    )
    .map_err(|err| GatewayError::Internal(err.to_string()))
}
