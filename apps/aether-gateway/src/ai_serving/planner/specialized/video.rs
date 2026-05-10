mod decision;
mod request;
mod support;

use async_trait::async_trait;
use tracing::warn;

use crate::ai_serving::planner::candidate_materialization::LocalExecutionAttemptSource;
use crate::ai_serving::planner::plan_builders::{
    build_passthrough_sync_plan_from_decision, AiSyncAttempt,
};
use crate::ai_serving::planner::spec_metadata::local_video_create_spec_metadata;
use crate::ai_serving::GatewayControlDecision;
use crate::ai_serving::{
    resolve_local_video_sync_spec as resolve_sync_spec, LocalVideoCreateFamily,
    LocalVideoCreateSpec,
};
use crate::{AiExecutionDecision, AppState, GatewayError};

use self::decision::maybe_build_local_video_create_decision_payload_for_candidate;
use self::support::{
    build_local_video_create_candidate_attempt_source, list_local_video_create_candidate_attempts,
    resolve_local_video_create_decision_input, LocalVideoCreateCandidateAttempt,
    LocalVideoCreateCandidateAttemptSource, LocalVideoCreateDecisionInput,
};

pub(crate) struct LocalVideoCreateSyncAttemptSource<'a> {
    state: &'a AppState,
    parts: &'a http::request::Parts,
    body_json: &'a serde_json::Value,
    trace_id: &'a str,
    input: LocalVideoCreateDecisionInput,
    spec: LocalVideoCreateSpec,
    candidates: LocalVideoCreateCandidateAttemptSource<'a>,
}

pub(crate) async fn build_local_video_sync_plan_and_reports_for_kind(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
) -> Result<Vec<AiSyncAttempt>, GatewayError> {
    let Some(spec) = resolve_sync_spec(plan_kind) else {
        return Ok(Vec::new());
    };

    build_local_sync_plan_and_reports(state, parts, body_json, trace_id, decision, spec).await
}

pub(crate) async fn build_local_video_sync_attempt_source_for_kind<'a>(
    state: &'a AppState,
    parts: &'a http::request::Parts,
    body_json: &'a serde_json::Value,
    trace_id: &'a str,
    decision: &'a GatewayControlDecision,
    plan_kind: &str,
) -> Result<Option<(LocalVideoCreateSyncAttemptSource<'a>, usize)>, GatewayError> {
    let Some(spec) = resolve_sync_spec(plan_kind) else {
        return Ok(None);
    };
    let spec_metadata = local_video_create_spec_metadata(spec);

    let Some(input) = resolve_local_video_create_decision_input(
        state, parts, trace_id, decision, body_json, spec,
    )
    .await
    else {
        return Ok(None);
    };

    let Some((candidates, candidate_count)) = build_local_video_create_candidate_attempt_source(
        state,
        trace_id,
        &input,
        body_json,
        spec_metadata.api_format,
        spec_metadata.decision_kind,
    )
    .await?
    else {
        return Ok(None);
    };

    if candidate_count == 0 {
        return Ok(None);
    }

    Ok(Some((
        LocalVideoCreateSyncAttemptSource {
            state,
            parts,
            body_json,
            trace_id,
            input,
            spec,
            candidates,
        },
        candidate_count,
    )))
}

#[async_trait]
impl LocalExecutionAttemptSource<AiSyncAttempt> for LocalVideoCreateSyncAttemptSource<'_> {
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

impl LocalVideoCreateSyncAttemptSource<'_> {
    async fn build_sync_attempt(
        &self,
        attempt: LocalVideoCreateCandidateAttempt,
    ) -> Result<Option<AiSyncAttempt>, GatewayError> {
        let spec_metadata = local_video_create_spec_metadata(self.spec);
        let Some(payload) = maybe_build_local_video_create_decision_payload_for_candidate(
            self.state,
            self.parts,
            self.body_json,
            self.trace_id,
            &self.input,
            attempt,
            self.spec,
        )
        .await
        else {
            return Ok(None);
        };

        match build_passthrough_sync_plan_from_decision(self.parts, payload) {
            Ok(value) => Ok(value),
            Err(err) => {
                warn!(
                    trace_id = %self.trace_id,
                    decision_kind = spec_metadata.decision_kind,
                    error = ?err,
                    "gateway local video sync decision plan build failed"
                );
                Ok(None)
            }
        }
    }
}

pub(crate) async fn maybe_build_sync_local_video_decision_payload(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
) -> Result<Option<AiExecutionDecision>, GatewayError> {
    let Some(spec) = resolve_sync_spec(plan_kind) else {
        return Ok(None);
    };
    let spec_metadata = local_video_create_spec_metadata(spec);

    let Some(input) = resolve_local_video_create_decision_input(
        state, parts, trace_id, decision, body_json, spec,
    )
    .await
    else {
        return Ok(None);
    };

    let Some(attempts) = list_local_video_create_candidate_attempts(
        state,
        trace_id,
        &input,
        body_json,
        spec_metadata.api_format,
        spec_metadata.decision_kind,
    )
    .await
    else {
        return Ok(None);
    };

    for attempt in attempts {
        if let Some(payload) = maybe_build_local_video_create_decision_payload_for_candidate(
            state, parts, body_json, trace_id, &input, attempt, spec,
        )
        .await
        {
            return Ok(Some(payload));
        }
    }

    Ok(None)
}

async fn build_local_sync_plan_and_reports(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    trace_id: &str,
    decision: &GatewayControlDecision,
    spec: LocalVideoCreateSpec,
) -> Result<Vec<AiSyncAttempt>, GatewayError> {
    let spec_metadata = local_video_create_spec_metadata(spec);
    let Some(input) = resolve_local_video_create_decision_input(
        state, parts, trace_id, decision, body_json, spec,
    )
    .await
    else {
        return Ok(Vec::new());
    };

    let Some(attempts) = list_local_video_create_candidate_attempts(
        state,
        trace_id,
        &input,
        body_json,
        spec_metadata.api_format,
        spec_metadata.decision_kind,
    )
    .await
    else {
        return Ok(Vec::new());
    };

    let mut plans = Vec::new();
    for attempt in attempts {
        let Some(payload) = maybe_build_local_video_create_decision_payload_for_candidate(
            state, parts, body_json, trace_id, &input, attempt, spec,
        )
        .await
        else {
            continue;
        };

        match build_passthrough_sync_plan_from_decision(parts, payload) {
            Ok(Some(value)) => plans.push(value),
            Ok(None) => {}
            Err(err) => {
                warn!(
                    trace_id = %trace_id,
                    decision_kind = spec_metadata.decision_kind,
                    error = ?err,
                    "gateway local video sync decision plan build failed"
                );
            }
        }
    }

    Ok(plans)
}
