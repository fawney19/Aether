use async_trait::async_trait;
use tracing::warn;

use super::decision::{
    build_local_openai_responses_candidate_attempt_source,
    materialize_local_openai_responses_candidate_attempts,
    maybe_build_local_openai_responses_decision_payload_for_candidate,
    resolve_local_openai_responses_decision_input, LocalOpenAiResponsesCandidateAttempt,
    LocalOpenAiResponsesCandidateAttemptSource, LocalOpenAiResponsesDecisionInput,
    LocalOpenAiResponsesSpec,
};
use crate::ai_serving::planner::candidate_materialization::LocalExecutionAttemptSource;
use crate::ai_serving::planner::plan_builders::{
    build_openai_responses_stream_plan_from_decision,
    build_openai_responses_sync_plan_from_decision, AiStreamAttempt, AiSyncAttempt,
};
use crate::ai_serving::planner::runtime_miss::{
    apply_local_runtime_candidate_evaluation_progress,
    apply_local_runtime_candidate_terminal_reason, set_local_runtime_miss_diagnostic_reason,
};
use crate::ai_serving::planner::spec_metadata::local_openai_responses_spec_metadata;
use crate::ai_serving::GatewayControlDecision;
pub(crate) use crate::ai_serving::{
    resolve_openai_responses_stream_spec as resolve_stream_spec,
    resolve_openai_responses_sync_spec as resolve_sync_spec,
};
use crate::{AppState, GatewayError};

pub(crate) struct LocalOpenAiResponsesSyncAttemptSource<'a> {
    state: &'a AppState,
    parts: &'a http::request::Parts,
    trace_id: &'a str,
    body_json: &'a serde_json::Value,
    input: LocalOpenAiResponsesDecisionInput,
    spec: LocalOpenAiResponsesSpec,
    candidates: LocalOpenAiResponsesCandidateAttemptSource<'a>,
}

pub(crate) struct LocalOpenAiResponsesStreamAttemptSource<'a> {
    state: &'a AppState,
    parts: &'a http::request::Parts,
    trace_id: &'a str,
    body_json: &'a serde_json::Value,
    input: LocalOpenAiResponsesDecisionInput,
    spec: LocalOpenAiResponsesSpec,
    candidates: LocalOpenAiResponsesCandidateAttemptSource<'a>,
}

pub(super) async fn build_local_sync_attempt_source<'a>(
    state: &'a AppState,
    parts: &'a http::request::Parts,
    trace_id: &'a str,
    decision: &'a GatewayControlDecision,
    body_json: &'a serde_json::Value,
    spec: LocalOpenAiResponsesSpec,
) -> Result<Option<(LocalOpenAiResponsesSyncAttemptSource<'a>, usize)>, GatewayError> {
    let spec_metadata = local_openai_responses_spec_metadata(spec);
    let Some(input) = resolve_local_openai_responses_decision_input(
        state,
        parts,
        trace_id,
        decision,
        body_json,
        spec_metadata.decision_kind,
    )
    .await
    else {
        return Ok(None);
    };
    set_local_runtime_miss_diagnostic_reason(
        state,
        trace_id,
        decision,
        spec_metadata.decision_kind,
        Some(input.requested_model.as_str()),
        "candidate_evaluation_incomplete",
    );
    let (candidates, candidate_count) = build_local_openai_responses_candidate_attempt_source(
        state, trace_id, &input, body_json, spec,
    )
    .await?;
    apply_local_runtime_candidate_evaluation_progress(state, trace_id, candidate_count);
    if candidate_count == 0 {
        return Ok(None);
    }

    Ok(Some((
        LocalOpenAiResponsesSyncAttemptSource {
            state,
            parts,
            trace_id,
            body_json,
            input,
            spec,
            candidates,
        },
        candidate_count,
    )))
}

pub(super) async fn build_local_stream_attempt_source<'a>(
    state: &'a AppState,
    parts: &'a http::request::Parts,
    trace_id: &'a str,
    decision: &'a GatewayControlDecision,
    body_json: &'a serde_json::Value,
    spec: LocalOpenAiResponsesSpec,
) -> Result<Option<(LocalOpenAiResponsesStreamAttemptSource<'a>, usize)>, GatewayError> {
    let spec_metadata = local_openai_responses_spec_metadata(spec);
    let Some(input) = resolve_local_openai_responses_decision_input(
        state,
        parts,
        trace_id,
        decision,
        body_json,
        spec_metadata.decision_kind,
    )
    .await
    else {
        return Ok(None);
    };
    set_local_runtime_miss_diagnostic_reason(
        state,
        trace_id,
        decision,
        spec_metadata.decision_kind,
        Some(input.requested_model.as_str()),
        "candidate_evaluation_incomplete",
    );
    let (candidates, candidate_count) = build_local_openai_responses_candidate_attempt_source(
        state, trace_id, &input, body_json, spec,
    )
    .await?;
    apply_local_runtime_candidate_evaluation_progress(state, trace_id, candidate_count);
    if candidate_count == 0 {
        return Ok(None);
    }

    Ok(Some((
        LocalOpenAiResponsesStreamAttemptSource {
            state,
            parts,
            trace_id,
            body_json,
            input,
            spec,
            candidates,
        },
        candidate_count,
    )))
}

#[async_trait]
impl LocalExecutionAttemptSource<AiSyncAttempt> for LocalOpenAiResponsesSyncAttemptSource<'_> {
    async fn promote_hedge_preheated_pool_candidates(
        &mut self,
        provider_id: &str,
        pool_group_id: &str,
        attempted_key_ids: &std::collections::BTreeSet<String>,
    ) -> Result<Option<(Option<String>, String, Vec<String>)>, GatewayError> {
        self.candidates
            .promote_hedge_preheated_pool_candidates(provider_id, pool_group_id, attempted_key_ids)
            .await
    }

    async fn next_execution_attempt(&mut self) -> Result<Option<AiSyncAttempt>, GatewayError> {
        while let Some(attempt) = self.candidates.next_attempt().await {
            match self.build_sync_attempt(attempt).await? {
                Some(attempt) => return Ok(Some(attempt)),
                None => continue,
            }
        }
        apply_local_runtime_candidate_terminal_reason(
            self.state,
            self.trace_id,
            "no_local_sync_plans",
        );
        Ok(None)
    }

    async fn drain_execution_attempts(&mut self) -> Result<Vec<AiSyncAttempt>, GatewayError> {
        let mut drained = Vec::new();
        for attempt in self.candidates.drain_static_attempts() {
            if let Some(attempt) = self.build_sync_attempt(attempt).await? {
                drained.push(attempt);
            }
        }
        Ok(drained)
    }
}

#[async_trait]
impl LocalExecutionAttemptSource<AiStreamAttempt> for LocalOpenAiResponsesStreamAttemptSource<'_> {
    async fn promote_hedge_preheated_pool_candidates(
        &mut self,
        provider_id: &str,
        pool_group_id: &str,
        attempted_key_ids: &std::collections::BTreeSet<String>,
    ) -> Result<Option<(Option<String>, String, Vec<String>)>, GatewayError> {
        self.candidates
            .promote_hedge_preheated_pool_candidates(provider_id, pool_group_id, attempted_key_ids)
            .await
    }

    async fn next_execution_attempt(&mut self) -> Result<Option<AiStreamAttempt>, GatewayError> {
        while let Some(attempt) = self.candidates.next_attempt().await {
            match self.build_stream_attempt(attempt).await? {
                Some(attempt) => return Ok(Some(attempt)),
                None => continue,
            }
        }
        apply_local_runtime_candidate_terminal_reason(
            self.state,
            self.trace_id,
            "no_local_stream_plans",
        );
        Ok(None)
    }

    async fn drain_execution_attempts(&mut self) -> Result<Vec<AiStreamAttempt>, GatewayError> {
        let mut drained = Vec::new();
        for attempt in self.candidates.drain_static_attempts() {
            if let Some(attempt) = self.build_stream_attempt(attempt).await? {
                drained.push(attempt);
            }
        }
        Ok(drained)
    }
}

impl LocalOpenAiResponsesSyncAttemptSource<'_> {
    async fn build_sync_attempt(
        &self,
        attempt: LocalOpenAiResponsesCandidateAttempt,
    ) -> Result<Option<AiSyncAttempt>, GatewayError> {
        let Some(payload) = maybe_build_local_openai_responses_decision_payload_for_candidate(
            self.state,
            self.parts,
            self.trace_id,
            self.body_json,
            &self.input,
            attempt,
            self.spec,
        )
        .await
        else {
            return Ok(None);
        };

        match build_openai_responses_sync_plan_from_decision(
            self.parts,
            self.body_json,
            payload,
            self.spec.compact,
        ) {
            Ok(value) => Ok(value),
            Err(err) => {
                warn!(
                    trace_id = %self.trace_id,
                    error = ?err,
                    "gateway local openai responses sync decision plan build failed"
                );
                Ok(None)
            }
        }
    }
}

impl LocalOpenAiResponsesStreamAttemptSource<'_> {
    async fn build_stream_attempt(
        &self,
        attempt: LocalOpenAiResponsesCandidateAttempt,
    ) -> Result<Option<AiStreamAttempt>, GatewayError> {
        let Some(payload) = maybe_build_local_openai_responses_decision_payload_for_candidate(
            self.state,
            self.parts,
            self.trace_id,
            self.body_json,
            &self.input,
            attempt,
            self.spec,
        )
        .await
        else {
            return Ok(None);
        };

        match build_openai_responses_stream_plan_from_decision(
            self.parts,
            self.body_json,
            payload,
            self.spec.compact,
        ) {
            Ok(value) => Ok(value),
            Err(err) => {
                warn!(
                    trace_id = %self.trace_id,
                    error = ?err,
                    "gateway local openai responses stream decision plan build failed"
                );
                Ok(None)
            }
        }
    }
}

pub(super) async fn build_local_sync_plan_and_reports(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    spec: LocalOpenAiResponsesSpec,
) -> Result<Vec<AiSyncAttempt>, GatewayError> {
    let spec_metadata = local_openai_responses_spec_metadata(spec);
    let Some(input) = resolve_local_openai_responses_decision_input(
        state,
        parts,
        trace_id,
        decision,
        body_json,
        spec_metadata.decision_kind,
    )
    .await
    else {
        return Ok(Vec::new());
    };
    set_local_runtime_miss_diagnostic_reason(
        state,
        trace_id,
        decision,
        spec_metadata.decision_kind,
        Some(input.requested_model.as_str()),
        "candidate_evaluation_incomplete",
    );

    let (attempts, candidate_count) = materialize_local_openai_responses_candidate_attempts(
        state, trace_id, &input, body_json, spec,
    )
    .await?;
    apply_local_runtime_candidate_evaluation_progress(state, trace_id, candidate_count);
    if candidate_count == 0 {
        return Ok(Vec::new());
    }

    let mut plans = Vec::new();
    for attempt in attempts {
        let Some(payload) = maybe_build_local_openai_responses_decision_payload_for_candidate(
            state, parts, trace_id, body_json, &input, attempt, spec,
        )
        .await
        else {
            continue;
        };

        match build_openai_responses_sync_plan_from_decision(
            parts,
            body_json,
            payload,
            spec.compact,
        ) {
            Ok(Some(value)) => plans.push(value),
            Ok(None) => {}
            Err(err) => {
                warn!(
                    trace_id = %trace_id,
                    api_format = spec_metadata.api_format,
                    error = ?err,
                    "gateway local openai responses sync decision plan build failed"
                );
            }
        }
    }

    apply_local_runtime_candidate_terminal_reason(state, trace_id, "no_local_sync_plans");
    Ok(plans)
}

pub(super) async fn build_local_stream_plan_and_reports(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    spec: LocalOpenAiResponsesSpec,
) -> Result<Vec<AiStreamAttempt>, GatewayError> {
    let spec_metadata = local_openai_responses_spec_metadata(spec);
    let Some(input) = resolve_local_openai_responses_decision_input(
        state,
        parts,
        trace_id,
        decision,
        body_json,
        spec_metadata.decision_kind,
    )
    .await
    else {
        return Ok(Vec::new());
    };
    set_local_runtime_miss_diagnostic_reason(
        state,
        trace_id,
        decision,
        spec_metadata.decision_kind,
        Some(input.requested_model.as_str()),
        "candidate_evaluation_incomplete",
    );

    let (attempts, candidate_count) = materialize_local_openai_responses_candidate_attempts(
        state, trace_id, &input, body_json, spec,
    )
    .await?;
    apply_local_runtime_candidate_evaluation_progress(state, trace_id, candidate_count);
    if candidate_count == 0 {
        return Ok(Vec::new());
    }

    let mut plans = Vec::new();
    for attempt in attempts {
        let Some(payload) = maybe_build_local_openai_responses_decision_payload_for_candidate(
            state, parts, trace_id, body_json, &input, attempt, spec,
        )
        .await
        else {
            continue;
        };

        match build_openai_responses_stream_plan_from_decision(
            parts,
            body_json,
            payload,
            spec.compact,
        ) {
            Ok(Some(value)) => plans.push(value),
            Ok(None) => {}
            Err(err) => {
                warn!(
                    trace_id = %trace_id,
                    api_format = spec_metadata.api_format,
                    error = ?err,
                    "gateway local openai responses stream decision plan build failed"
                );
            }
        }
    }

    apply_local_runtime_candidate_terminal_reason(state, trace_id, "no_local_stream_plans");
    Ok(plans)
}
