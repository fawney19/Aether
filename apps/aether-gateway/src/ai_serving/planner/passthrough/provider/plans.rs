use async_trait::async_trait;
use tracing::warn;

use crate::ai_serving::planner::candidate_materialization::LocalExecutionAttemptSource;
use crate::ai_serving::planner::common::{
    extract_requested_model_from_request, RequestedModelFamily,
};
use crate::ai_serving::planner::runtime_miss::{
    apply_local_runtime_candidate_evaluation_progress_preserving_candidate_signal,
    apply_local_runtime_candidate_terminal_reason, set_local_runtime_miss_diagnostic_reason,
};
use crate::ai_serving::planner::spec_metadata::{
    build_stream_plan_from_requested_model_family, build_sync_plan_from_requested_model_family,
    local_same_format_provider_spec_metadata,
};
pub(crate) use crate::ai_serving::{
    resolve_local_same_format_stream_spec as resolve_stream_spec,
    resolve_local_same_format_sync_spec as resolve_sync_spec,
};

use super::{
    build_local_same_format_provider_candidate_attempt_source,
    materialize_local_same_format_provider_candidate_attempts,
    maybe_build_local_same_format_provider_decision_payload_for_candidate,
    resolve_local_same_format_provider_decision_input, AiStreamAttempt, AiSyncAttempt, AppState,
    GatewayControlDecision, GatewayError, LocalSameFormatProviderCandidateAttempt,
    LocalSameFormatProviderCandidateAttemptSource, LocalSameFormatProviderDecisionInput,
    LocalSameFormatProviderSpec,
};

pub(crate) struct LocalSameFormatProviderSyncAttemptSource<'a> {
    state: &'a AppState,
    parts: &'a http::request::Parts,
    trace_id: &'a str,
    body_json: &'a serde_json::Value,
    input: LocalSameFormatProviderDecisionInput,
    spec: LocalSameFormatProviderSpec,
    requested_model_family: RequestedModelFamily,
    candidates: LocalSameFormatProviderCandidateAttemptSource<'a>,
}

pub(crate) struct LocalSameFormatProviderStreamAttemptSource<'a> {
    state: &'a AppState,
    parts: &'a http::request::Parts,
    trace_id: &'a str,
    body_json: &'a serde_json::Value,
    input: LocalSameFormatProviderDecisionInput,
    spec: LocalSameFormatProviderSpec,
    requested_model_family: RequestedModelFamily,
    candidates: LocalSameFormatProviderCandidateAttemptSource<'a>,
}

pub(crate) async fn build_local_sync_attempt_source<'a>(
    state: &'a AppState,
    parts: &'a http::request::Parts,
    trace_id: &'a str,
    decision: &'a GatewayControlDecision,
    body_json: &'a serde_json::Value,
    spec: LocalSameFormatProviderSpec,
) -> Result<Option<(LocalSameFormatProviderSyncAttemptSource<'a>, usize)>, GatewayError> {
    let spec_metadata = local_same_format_provider_spec_metadata(spec);
    let requested_model_family = spec_metadata
        .requested_model_family
        .expect("same-format provider spec metadata should include requested-model family");
    let Some(input) = resolve_local_same_format_provider_decision_input(
        state, parts, trace_id, decision, body_json, spec,
    )
    .await
    else {
        set_local_runtime_miss_diagnostic_reason(
            state,
            trace_id,
            decision,
            spec_metadata.decision_kind,
            extract_requested_model_from_request(parts, body_json, requested_model_family)
                .as_deref(),
            "decision_input_unavailable",
        );
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
    let (candidates, candidate_count) = build_local_same_format_provider_candidate_attempt_source(
        state, trace_id, &input, body_json, spec,
    )
    .await?;
    apply_local_runtime_candidate_evaluation_progress_preserving_candidate_signal(
        state,
        trace_id,
        candidate_count,
    );
    if candidate_count == 0 {
        return Ok(None);
    }

    Ok(Some((
        LocalSameFormatProviderSyncAttemptSource {
            state,
            parts,
            trace_id,
            body_json,
            input,
            spec,
            requested_model_family,
            candidates,
        },
        candidate_count,
    )))
}

pub(crate) async fn build_local_stream_attempt_source<'a>(
    state: &'a AppState,
    parts: &'a http::request::Parts,
    trace_id: &'a str,
    decision: &'a GatewayControlDecision,
    body_json: &'a serde_json::Value,
    spec: LocalSameFormatProviderSpec,
) -> Result<Option<(LocalSameFormatProviderStreamAttemptSource<'a>, usize)>, GatewayError> {
    let spec_metadata = local_same_format_provider_spec_metadata(spec);
    let requested_model_family = spec_metadata
        .requested_model_family
        .expect("same-format provider spec metadata should include requested-model family");
    let Some(input) = resolve_local_same_format_provider_decision_input(
        state, parts, trace_id, decision, body_json, spec,
    )
    .await
    else {
        set_local_runtime_miss_diagnostic_reason(
            state,
            trace_id,
            decision,
            spec_metadata.decision_kind,
            extract_requested_model_from_request(parts, body_json, requested_model_family)
                .as_deref(),
            "decision_input_unavailable",
        );
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
    let (candidates, candidate_count) = build_local_same_format_provider_candidate_attempt_source(
        state, trace_id, &input, body_json, spec,
    )
    .await?;
    apply_local_runtime_candidate_evaluation_progress_preserving_candidate_signal(
        state,
        trace_id,
        candidate_count,
    );
    if candidate_count == 0 {
        return Ok(None);
    }

    Ok(Some((
        LocalSameFormatProviderStreamAttemptSource {
            state,
            parts,
            trace_id,
            body_json,
            input,
            spec,
            requested_model_family,
            candidates,
        },
        candidate_count,
    )))
}

#[async_trait]
impl LocalExecutionAttemptSource<AiSyncAttempt> for LocalSameFormatProviderSyncAttemptSource<'_> {
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
impl LocalExecutionAttemptSource<AiStreamAttempt>
    for LocalSameFormatProviderStreamAttemptSource<'_>
{
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

impl LocalSameFormatProviderSyncAttemptSource<'_> {
    async fn build_sync_attempt(
        &self,
        attempt: LocalSameFormatProviderCandidateAttempt,
    ) -> Result<Option<AiSyncAttempt>, GatewayError> {
        let Some(payload) = maybe_build_local_same_format_provider_decision_payload_for_candidate(
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

        match build_sync_plan_from_requested_model_family(
            self.requested_model_family,
            self.parts,
            self.body_json,
            payload,
        ) {
            Ok(value) => Ok(value),
            Err(err) => {
                warn!(
                    trace_id = %self.trace_id,
                    error = ?err,
                    "gateway local same-format sync decision plan build failed"
                );
                Ok(None)
            }
        }
    }
}

impl LocalSameFormatProviderStreamAttemptSource<'_> {
    async fn build_stream_attempt(
        &self,
        attempt: LocalSameFormatProviderCandidateAttempt,
    ) -> Result<Option<AiStreamAttempt>, GatewayError> {
        let Some(payload) = maybe_build_local_same_format_provider_decision_payload_for_candidate(
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

        match build_stream_plan_from_requested_model_family(
            self.requested_model_family,
            self.parts,
            self.body_json,
            payload,
        ) {
            Ok(value) => Ok(value),
            Err(err) => {
                warn!(
                    trace_id = %self.trace_id,
                    error = ?err,
                    "gateway local same-format stream decision plan build failed"
                );
                Ok(None)
            }
        }
    }
}

pub(crate) async fn build_local_sync_plan_and_reports(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    spec: LocalSameFormatProviderSpec,
) -> Result<Vec<AiSyncAttempt>, GatewayError> {
    let spec_metadata = local_same_format_provider_spec_metadata(spec);
    let requested_model_family = spec_metadata
        .requested_model_family
        .expect("same-format provider spec metadata should include requested-model family");
    let Some(input) = resolve_local_same_format_provider_decision_input(
        state, parts, trace_id, decision, body_json, spec,
    )
    .await
    else {
        set_local_runtime_miss_diagnostic_reason(
            state,
            trace_id,
            decision,
            spec_metadata.decision_kind,
            extract_requested_model_from_request(parts, body_json, requested_model_family)
                .as_deref(),
            "decision_input_unavailable",
        );
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
    let (attempts, candidate_count) = materialize_local_same_format_provider_candidate_attempts(
        state, trace_id, &input, body_json, spec,
    )
    .await?;
    apply_local_runtime_candidate_evaluation_progress_preserving_candidate_signal(
        state,
        trace_id,
        candidate_count,
    );
    if candidate_count == 0 {
        return Ok(Vec::new());
    }

    let mut plans = Vec::new();
    for attempt in attempts {
        let Some(payload) = maybe_build_local_same_format_provider_decision_payload_for_candidate(
            state, parts, trace_id, body_json, &input, attempt, spec,
        )
        .await
        else {
            continue;
        };

        let built = build_sync_plan_from_requested_model_family(
            requested_model_family,
            parts,
            body_json,
            payload,
        );

        match built {
            Ok(Some(value)) => plans.push(value),
            Ok(None) => {}
            Err(err) => {
                warn!(
                    trace_id = %trace_id,
                    api_format = spec_metadata.api_format,
                    error = ?err,
                    "gateway local same-format sync decision plan build failed"
                );
            }
        }
    }

    apply_local_runtime_candidate_terminal_reason(state, trace_id, "no_local_sync_plans");

    Ok(plans)
}

pub(crate) async fn build_local_stream_plan_and_reports(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    spec: LocalSameFormatProviderSpec,
) -> Result<Vec<AiStreamAttempt>, GatewayError> {
    let spec_metadata = local_same_format_provider_spec_metadata(spec);
    let requested_model_family = spec_metadata
        .requested_model_family
        .expect("same-format provider spec metadata should include requested-model family");
    let Some(input) = resolve_local_same_format_provider_decision_input(
        state, parts, trace_id, decision, body_json, spec,
    )
    .await
    else {
        set_local_runtime_miss_diagnostic_reason(
            state,
            trace_id,
            decision,
            spec_metadata.decision_kind,
            extract_requested_model_from_request(parts, body_json, requested_model_family)
                .as_deref(),
            "decision_input_unavailable",
        );
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
    let (attempts, candidate_count) = materialize_local_same_format_provider_candidate_attempts(
        state, trace_id, &input, body_json, spec,
    )
    .await?;
    apply_local_runtime_candidate_evaluation_progress_preserving_candidate_signal(
        state,
        trace_id,
        candidate_count,
    );
    if candidate_count == 0 {
        return Ok(Vec::new());
    }

    let mut plans = Vec::new();
    for attempt in attempts {
        let Some(payload) = maybe_build_local_same_format_provider_decision_payload_for_candidate(
            state, parts, trace_id, body_json, &input, attempt, spec,
        )
        .await
        else {
            continue;
        };

        let built = build_stream_plan_from_requested_model_family(
            requested_model_family,
            parts,
            body_json,
            payload,
        );

        match built {
            Ok(Some(value)) => plans.push(value),
            Ok(None) => {}
            Err(err) => {
                warn!(
                    trace_id = %trace_id,
                    api_format = spec_metadata.api_format,
                    error = ?err,
                    "gateway local same-format stream decision plan build failed"
                );
            }
        }
    }

    apply_local_runtime_candidate_terminal_reason(state, trace_id, "no_local_stream_plans");

    Ok(plans)
}
