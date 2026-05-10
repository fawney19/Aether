mod decision;
mod request;
mod support;

use async_trait::async_trait;
use tracing::warn;

use crate::ai_serving::planner::candidate_materialization::LocalExecutionAttemptSource;
use crate::ai_serving::planner::plan_builders::{
    build_passthrough_stream_plan_from_decision, build_passthrough_sync_plan_from_decision,
    AiStreamAttempt, AiSyncAttempt,
};
use crate::ai_serving::planner::spec_metadata::local_gemini_files_spec_metadata;
use crate::ai_serving::GatewayControlDecision;
use crate::ai_serving::{
    resolve_gemini_files_stream_spec as resolve_stream_spec,
    resolve_gemini_files_sync_spec as resolve_sync_spec, LocalGeminiFilesSpec,
};
use crate::{AiExecutionDecision, AppState, GatewayError};

use self::decision::maybe_build_local_gemini_files_decision_payload_for_candidate;
use self::support::{
    build_local_gemini_files_candidate_attempt_source,
    materialize_local_gemini_files_candidate_attempts, resolve_local_gemini_files_decision_input,
    LocalGeminiFilesCandidateAttempt, LocalGeminiFilesCandidateAttemptSource,
    LocalGeminiFilesDecisionInput,
};

pub(crate) struct LocalGeminiFilesSyncAttemptSource<'a> {
    state: &'a AppState,
    parts: &'a http::request::Parts,
    body_json: &'a serde_json::Value,
    body_base64: Option<&'a str>,
    body_is_empty: bool,
    trace_id: &'a str,
    input: LocalGeminiFilesDecisionInput,
    spec: LocalGeminiFilesSpec,
    candidates: LocalGeminiFilesCandidateAttemptSource<'a>,
}

pub(crate) struct LocalGeminiFilesStreamAttemptSource<'a> {
    state: &'a AppState,
    parts: &'a http::request::Parts,
    trace_id: &'a str,
    input: LocalGeminiFilesDecisionInput,
    spec: LocalGeminiFilesSpec,
    candidates: LocalGeminiFilesCandidateAttemptSource<'a>,
}

pub(crate) async fn build_local_gemini_files_sync_plan_and_reports_for_kind(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    body_base64: Option<&str>,
    body_is_empty: bool,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
) -> Result<Vec<AiSyncAttempt>, GatewayError> {
    let Some(spec) = resolve_sync_spec(plan_kind) else {
        return Ok(Vec::new());
    };

    build_local_sync_plan_and_reports(
        state,
        parts,
        body_json,
        body_base64,
        body_is_empty,
        trace_id,
        decision,
        spec,
    )
    .await
}

pub(crate) async fn build_local_gemini_files_stream_plan_and_reports_for_kind(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
) -> Result<Vec<AiStreamAttempt>, GatewayError> {
    let Some(spec) = resolve_stream_spec(plan_kind) else {
        return Ok(Vec::new());
    };

    build_local_stream_plan_and_reports(state, parts, trace_id, decision, spec).await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn build_local_gemini_files_sync_attempt_source_for_kind<'a>(
    state: &'a AppState,
    parts: &'a http::request::Parts,
    body_json: &'a serde_json::Value,
    body_base64: Option<&'a str>,
    body_is_empty: bool,
    trace_id: &'a str,
    decision: &'a GatewayControlDecision,
    plan_kind: &str,
) -> Result<Option<(LocalGeminiFilesSyncAttemptSource<'a>, usize)>, GatewayError> {
    let Some(spec) = resolve_sync_spec(plan_kind) else {
        return Ok(None);
    };

    let Some(input) = resolve_local_gemini_files_decision_input(
        state,
        parts,
        Some(body_json),
        trace_id,
        decision,
    )
    .await
    else {
        return Ok(None);
    };
    let (candidates, candidate_count) =
        build_local_gemini_files_candidate_attempt_source(state, trace_id, &input).await?;
    if candidate_count == 0 {
        return Ok(None);
    }

    Ok(Some((
        LocalGeminiFilesSyncAttemptSource {
            state,
            parts,
            body_json,
            body_base64,
            body_is_empty,
            trace_id,
            input,
            spec,
            candidates,
        },
        candidate_count,
    )))
}

pub(crate) async fn build_local_gemini_files_stream_attempt_source_for_kind<'a>(
    state: &'a AppState,
    parts: &'a http::request::Parts,
    trace_id: &'a str,
    decision: &'a GatewayControlDecision,
    plan_kind: &str,
) -> Result<Option<(LocalGeminiFilesStreamAttemptSource<'a>, usize)>, GatewayError> {
    let Some(spec) = resolve_stream_spec(plan_kind) else {
        return Ok(None);
    };

    let Some(input) =
        resolve_local_gemini_files_decision_input(state, parts, None, trace_id, decision).await
    else {
        return Ok(None);
    };
    let (candidates, candidate_count) =
        build_local_gemini_files_candidate_attempt_source(state, trace_id, &input).await?;
    if candidate_count == 0 {
        return Ok(None);
    }

    Ok(Some((
        LocalGeminiFilesStreamAttemptSource {
            state,
            parts,
            trace_id,
            input,
            spec,
            candidates,
        },
        candidate_count,
    )))
}

#[async_trait]
impl LocalExecutionAttemptSource<AiSyncAttempt> for LocalGeminiFilesSyncAttemptSource<'_> {
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

#[async_trait]
impl LocalExecutionAttemptSource<AiStreamAttempt> for LocalGeminiFilesStreamAttemptSource<'_> {
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

impl LocalGeminiFilesSyncAttemptSource<'_> {
    async fn build_sync_attempt(
        &self,
        attempt: LocalGeminiFilesCandidateAttempt,
    ) -> Result<Option<AiSyncAttempt>, GatewayError> {
        let spec_metadata = local_gemini_files_spec_metadata(self.spec);
        let Some(payload) = maybe_build_local_gemini_files_decision_payload_for_candidate(
            self.state,
            self.parts,
            self.body_json,
            self.body_base64,
            self.body_is_empty,
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
                    "gateway local gemini files sync decision plan build failed"
                );
                Ok(None)
            }
        }
    }
}

impl LocalGeminiFilesStreamAttemptSource<'_> {
    async fn build_stream_attempt(
        &self,
        attempt: LocalGeminiFilesCandidateAttempt,
    ) -> Result<Option<AiStreamAttempt>, GatewayError> {
        let spec_metadata = local_gemini_files_spec_metadata(self.spec);
        let empty_body_json = serde_json::Value::Null;
        let Some(payload) = maybe_build_local_gemini_files_decision_payload_for_candidate(
            self.state,
            self.parts,
            &empty_body_json,
            None,
            true,
            self.trace_id,
            &self.input,
            attempt,
            self.spec,
        )
        .await
        else {
            return Ok(None);
        };

        match build_passthrough_stream_plan_from_decision(self.parts, payload) {
            Ok(value) => Ok(value),
            Err(err) => {
                warn!(
                    trace_id = %self.trace_id,
                    decision_kind = spec_metadata.decision_kind,
                    error = ?err,
                    "gateway local gemini files stream decision plan build failed"
                );
                Ok(None)
            }
        }
    }
}

pub(crate) async fn maybe_build_sync_local_gemini_files_decision_payload(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    body_base64: Option<&str>,
    body_is_empty: bool,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
) -> Result<Option<AiExecutionDecision>, GatewayError> {
    let Some(spec) = resolve_sync_spec(plan_kind) else {
        return Ok(None);
    };

    let Some(input) = resolve_local_gemini_files_decision_input(
        state,
        parts,
        Some(body_json),
        trace_id,
        decision,
    )
    .await
    else {
        return Ok(None);
    };

    let attempts =
        materialize_local_gemini_files_candidate_attempts(state, trace_id, &input).await?;

    for attempt in attempts {
        if let Some(payload) = maybe_build_local_gemini_files_decision_payload_for_candidate(
            state,
            parts,
            body_json,
            body_base64,
            body_is_empty,
            trace_id,
            &input,
            attempt,
            spec,
        )
        .await
        {
            return Ok(Some(payload));
        }
    }

    Ok(None)
}

pub(crate) async fn maybe_build_stream_local_gemini_files_decision_payload(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
) -> Result<Option<AiExecutionDecision>, GatewayError> {
    let Some(spec) = resolve_stream_spec(plan_kind) else {
        return Ok(None);
    };

    let Some(input) =
        resolve_local_gemini_files_decision_input(state, parts, None, trace_id, decision).await
    else {
        return Ok(None);
    };

    let attempts =
        materialize_local_gemini_files_candidate_attempts(state, trace_id, &input).await?;

    let empty_body_json = serde_json::Value::Null;
    for attempt in attempts {
        if let Some(payload) = maybe_build_local_gemini_files_decision_payload_for_candidate(
            state,
            parts,
            &empty_body_json,
            None,
            true,
            trace_id,
            &input,
            attempt,
            spec,
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
    body_base64: Option<&str>,
    body_is_empty: bool,
    trace_id: &str,
    decision: &GatewayControlDecision,
    spec: LocalGeminiFilesSpec,
) -> Result<Vec<AiSyncAttempt>, GatewayError> {
    let spec_metadata = local_gemini_files_spec_metadata(spec);
    let Some(input) = resolve_local_gemini_files_decision_input(
        state,
        parts,
        Some(body_json),
        trace_id,
        decision,
    )
    .await
    else {
        return Ok(Vec::new());
    };

    let attempts =
        materialize_local_gemini_files_candidate_attempts(state, trace_id, &input).await?;

    let mut plans = Vec::new();
    for attempt in attempts {
        let Some(payload) = maybe_build_local_gemini_files_decision_payload_for_candidate(
            state,
            parts,
            body_json,
            body_base64,
            body_is_empty,
            trace_id,
            &input,
            attempt,
            spec,
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
                    "gateway local gemini files sync decision plan build failed"
                );
            }
        }
    }

    Ok(plans)
}

async fn build_local_stream_plan_and_reports(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    spec: LocalGeminiFilesSpec,
) -> Result<Vec<AiStreamAttempt>, GatewayError> {
    let spec_metadata = local_gemini_files_spec_metadata(spec);
    let Some(input) =
        resolve_local_gemini_files_decision_input(state, parts, None, trace_id, decision).await
    else {
        return Ok(Vec::new());
    };

    let attempts =
        materialize_local_gemini_files_candidate_attempts(state, trace_id, &input).await?;

    let mut plans = Vec::new();
    let empty_body_json = serde_json::Value::Null;
    for attempt in attempts {
        let Some(payload) = maybe_build_local_gemini_files_decision_payload_for_candidate(
            state,
            parts,
            &empty_body_json,
            None,
            true,
            trace_id,
            &input,
            attempt,
            spec,
        )
        .await
        else {
            continue;
        };

        match build_passthrough_stream_plan_from_decision(parts, payload) {
            Ok(Some(value)) => plans.push(value),
            Ok(None) => {}
            Err(err) => {
                warn!(
                    trace_id = %trace_id,
                    decision_kind = spec_metadata.decision_kind,
                    error = ?err,
                    "gateway local gemini files stream decision plan build failed"
                );
            }
        }
    }

    Ok(plans)
}
