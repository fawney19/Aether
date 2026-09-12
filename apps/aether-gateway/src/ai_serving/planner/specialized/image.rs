mod decision;
mod request;
mod support;

use async_trait::async_trait;
use tracing::warn;

use crate::ai_serving::planner::candidate_materialization::LocalExecutionAttemptSource;
use crate::ai_serving::planner::plan_builders::{
    build_gemini_stream_plan_from_decision, build_gemini_sync_plan_from_decision,
    build_passthrough_sync_plan_from_decision, build_standard_stream_plan_from_decision,
    AiStreamAttempt, AiSyncAttempt,
};
use crate::ai_serving::planner::runtime_miss::set_local_runtime_execution_exhausted_diagnostic;
use crate::ai_serving::planner::spec_metadata::local_openai_image_spec_metadata;
use crate::ai_serving::GatewayControlDecision;
use crate::ai_serving::{
    resolve_local_image_stream_spec as resolve_stream_spec,
    resolve_local_image_sync_spec as resolve_sync_spec,
};
use crate::{AiExecutionDecision, AppState, GatewayError};
use aether_routing_core::RoutingExecutionPolicy;

use self::decision::maybe_build_local_openai_image_decision_payload_for_candidate;
use self::support::{
    build_local_openai_image_candidate_attempt_source, list_local_openai_image_candidate_attempts,
    resolve_local_openai_image_decision_input, LocalOpenAiImageCandidateAttempt,
    LocalOpenAiImageCandidateAttemptSource, LocalOpenAiImageDecisionInput,
};

pub(super) use crate::ai_serving::LocalOpenAiImageSpec;

pub(crate) struct LocalOpenAiImageSyncAttemptSource<'a> {
    state: &'a AppState,
    parts: &'a http::request::Parts,
    body_json: serde_json::Value,
    body_base64: Option<&'a str>,
    trace_id: &'a str,
    input: LocalOpenAiImageDecisionInput,
    spec: LocalOpenAiImageSpec,
    candidates: LocalOpenAiImageCandidateAttemptSource<'a>,
}

pub(crate) struct LocalOpenAiImageStreamAttemptSource<'a> {
    state: &'a AppState,
    parts: &'a http::request::Parts,
    body_json: serde_json::Value,
    body_base64: Option<&'a str>,
    trace_id: &'a str,
    input: LocalOpenAiImageDecisionInput,
    spec: LocalOpenAiImageSpec,
    candidates: LocalOpenAiImageCandidateAttemptSource<'a>,
}

pub(crate) fn set_local_openai_image_execution_exhausted_diagnostic(
    state: &AppState,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
    body_json: &serde_json::Value,
    candidate_count: usize,
) {
    warn!(
        event_name = "local_openai_image_candidates_exhausted",
        log_type = "event",
        trace_id = %trace_id,
        plan_kind,
        route_class = decision.route_class.as_deref().unwrap_or("passthrough"),
        route_family = decision.route_family.as_deref().unwrap_or("unknown"),
        candidate_count,
        model = body_json.get("model").and_then(|value| value.as_str()).unwrap_or(""),
        "gateway local openai image execution exhausted all candidates"
    );
    set_local_runtime_execution_exhausted_diagnostic(
        state,
        trace_id,
        decision,
        plan_kind,
        body_json.get("model").and_then(|value| value.as_str()),
        candidate_count,
    );
}

pub(crate) async fn build_local_image_sync_plan_and_reports_for_kind(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    body_base64: Option<&str>,
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
        trace_id,
        decision,
        spec,
    )
    .await
}

pub(crate) async fn build_local_image_stream_plan_and_reports_for_kind(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    body_base64: Option<&str>,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
) -> Result<Vec<AiStreamAttempt>, GatewayError> {
    let Some(spec) = resolve_stream_spec(plan_kind) else {
        return Ok(Vec::new());
    };

    build_local_stream_plan_and_reports(
        state,
        parts,
        body_json,
        body_base64,
        trace_id,
        decision,
        spec,
    )
    .await
}

pub(crate) async fn build_local_image_sync_attempt_source_for_kind<'a>(
    state: &'a AppState,
    parts: &'a http::request::Parts,
    body_json: &'a serde_json::Value,
    body_base64: Option<&'a str>,
    trace_id: &'a str,
    decision: &'a GatewayControlDecision,
    plan_kind: &str,
) -> Result<Option<(LocalOpenAiImageSyncAttemptSource<'a>, usize)>, GatewayError> {
    let Some(spec) = resolve_sync_spec(plan_kind) else {
        return Ok(None);
    };
    let spec_metadata = local_openai_image_spec_metadata(spec);

    let Some(input) = resolve_local_openai_image_decision_input(
        state,
        parts,
        body_json,
        body_base64,
        trace_id,
        decision,
    )
    .await?
    else {
        return Ok(None);
    };

    let effective_body_json = input.effective_body_json(body_json).clone();
    let Some((candidates, candidate_count)) = build_local_openai_image_candidate_attempt_source(
        state,
        trace_id,
        &input,
        &effective_body_json,
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
        LocalOpenAiImageSyncAttemptSource {
            state,
            parts,
            body_json: effective_body_json,
            body_base64,
            trace_id,
            input,
            spec,
            candidates,
        },
        candidate_count,
    )))
}

pub(crate) async fn build_local_image_stream_attempt_source_for_kind<'a>(
    state: &'a AppState,
    parts: &'a http::request::Parts,
    body_json: &'a serde_json::Value,
    body_base64: Option<&'a str>,
    trace_id: &'a str,
    decision: &'a GatewayControlDecision,
    plan_kind: &str,
) -> Result<Option<(LocalOpenAiImageStreamAttemptSource<'a>, usize)>, GatewayError> {
    let Some(spec) = resolve_stream_spec(plan_kind) else {
        return Ok(None);
    };
    let spec_metadata = local_openai_image_spec_metadata(spec);

    let Some(input) = resolve_local_openai_image_decision_input(
        state,
        parts,
        body_json,
        body_base64,
        trace_id,
        decision,
    )
    .await?
    else {
        return Ok(None);
    };

    let effective_body_json = input.effective_body_json(body_json).clone();
    let Some((candidates, candidate_count)) = build_local_openai_image_candidate_attempt_source(
        state,
        trace_id,
        &input,
        &effective_body_json,
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
        LocalOpenAiImageStreamAttemptSource {
            state,
            parts,
            body_json: effective_body_json,
            body_base64,
            trace_id,
            input,
            spec,
            candidates,
        },
        candidate_count,
    )))
}

#[async_trait]
impl LocalExecutionAttemptSource<AiSyncAttempt> for LocalOpenAiImageSyncAttemptSource<'_> {
    fn routing_execution_policy(&self) -> Option<RoutingExecutionPolicy> {
        self.input
            .routing_policy
            .as_ref()
            .map(|policy| policy.execution_policy.clone())
    }

    async fn next_execution_attempt(&mut self) -> Result<Option<AiSyncAttempt>, GatewayError> {
        while let Some(attempt) = self.candidates.next_attempt().await? {
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

    async fn skip_credential(&mut self, key_id: &str) -> Result<(), GatewayError> {
        self.candidates.skip_credential(key_id);
        Ok(())
    }

    async fn skip_endpoint(&mut self, endpoint_id: &str) -> Result<(), GatewayError> {
        self.candidates.skip_endpoint(endpoint_id);
        Ok(())
    }

    async fn skip_provider(&mut self, provider_id: &str) -> Result<(), GatewayError> {
        self.candidates.skip_provider(provider_id);
        Ok(())
    }
}

#[async_trait]
impl LocalExecutionAttemptSource<AiStreamAttempt> for LocalOpenAiImageStreamAttemptSource<'_> {
    fn routing_execution_policy(&self) -> Option<RoutingExecutionPolicy> {
        self.input
            .routing_policy
            .as_ref()
            .map(|policy| policy.execution_policy.clone())
    }

    async fn next_execution_attempt(&mut self) -> Result<Option<AiStreamAttempt>, GatewayError> {
        while let Some(attempt) = self.candidates.next_attempt().await? {
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

    async fn skip_credential(&mut self, key_id: &str) -> Result<(), GatewayError> {
        self.candidates.skip_credential(key_id);
        Ok(())
    }

    async fn skip_endpoint(&mut self, endpoint_id: &str) -> Result<(), GatewayError> {
        self.candidates.skip_endpoint(endpoint_id);
        Ok(())
    }

    async fn skip_provider(&mut self, provider_id: &str) -> Result<(), GatewayError> {
        self.candidates.skip_provider(provider_id);
        Ok(())
    }
}

impl LocalOpenAiImageSyncAttemptSource<'_> {
    async fn build_sync_attempt(
        &self,
        attempt: LocalOpenAiImageCandidateAttempt,
    ) -> Result<Option<AiSyncAttempt>, GatewayError> {
        let spec_metadata = local_openai_image_spec_metadata(self.spec);
        let Some(payload) = maybe_build_local_openai_image_decision_payload_for_candidate(
            self.state,
            self.parts,
            &self.body_json,
            self.body_base64,
            self.trace_id,
            &self.input,
            attempt,
            self.spec,
        )
        .await?
        else {
            return Ok(None);
        };

        let provider_api_format = payload.provider_api_format.as_deref().unwrap_or_default();
        let built = if provider_api_format == "gemini:generate_content" {
            build_gemini_sync_plan_from_decision(self.parts, &self.body_json, payload)
        } else {
            build_passthrough_sync_plan_from_decision(self.parts, payload)
        };
        match built {
            Ok(value) => Ok(value),
            Err(err) => {
                warn!(
                    trace_id = %self.trace_id,
                    decision_kind = spec_metadata.decision_kind,
                    error = ?err,
                    "gateway local openai image sync decision plan build failed"
                );
                Ok(None)
            }
        }
    }
}

impl LocalOpenAiImageStreamAttemptSource<'_> {
    async fn build_stream_attempt(
        &self,
        attempt: LocalOpenAiImageCandidateAttempt,
    ) -> Result<Option<AiStreamAttempt>, GatewayError> {
        let spec_metadata = local_openai_image_spec_metadata(self.spec);
        let Some(payload) = maybe_build_local_openai_image_decision_payload_for_candidate(
            self.state,
            self.parts,
            &self.body_json,
            self.body_base64,
            self.trace_id,
            &self.input,
            attempt,
            self.spec,
        )
        .await?
        else {
            return Ok(None);
        };

        let provider_api_format = payload.provider_api_format.as_deref().unwrap_or_default();
        let built = if provider_api_format == "gemini:generate_content" {
            build_gemini_stream_plan_from_decision(self.parts, &self.body_json, payload)
        } else {
            build_standard_stream_plan_from_decision(self.parts, &self.body_json, payload, false)
        };
        match built {
            Ok(value) => Ok(value),
            Err(err) => {
                warn!(
                    trace_id = %self.trace_id,
                    decision_kind = spec_metadata.decision_kind,
                    error = ?err,
                    "gateway local openai image stream decision plan build failed"
                );
                Ok(None)
            }
        }
    }
}

pub(crate) async fn maybe_build_sync_local_image_decision_payload(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    body_base64: Option<&str>,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
) -> Result<Option<AiExecutionDecision>, GatewayError> {
    let Some(spec) = resolve_sync_spec(plan_kind) else {
        return Ok(None);
    };
    let spec_metadata = local_openai_image_spec_metadata(spec);

    let Some(input) = resolve_local_openai_image_decision_input(
        state,
        parts,
        body_json,
        body_base64,
        trace_id,
        decision,
    )
    .await?
    else {
        return Ok(None);
    };
    let body_json = input.effective_body_json(body_json);

    let Some((mut source, _)) = build_local_openai_image_candidate_attempt_source(
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

    while let Some(attempt) = source.next_attempt().await? {
        if let Some(payload) = maybe_build_local_openai_image_decision_payload_for_candidate(
            state,
            parts,
            body_json,
            body_base64,
            trace_id,
            &input,
            attempt,
            spec,
        )
        .await?
        {
            return Ok(Some(payload));
        }
    }

    Ok(None)
}

pub(crate) async fn maybe_build_stream_local_image_decision_payload(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    body_base64: Option<&str>,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
) -> Result<Option<AiExecutionDecision>, GatewayError> {
    let Some(spec) = resolve_stream_spec(plan_kind) else {
        return Ok(None);
    };
    let spec_metadata = local_openai_image_spec_metadata(spec);

    let Some(input) = resolve_local_openai_image_decision_input(
        state,
        parts,
        body_json,
        body_base64,
        trace_id,
        decision,
    )
    .await?
    else {
        return Ok(None);
    };
    let body_json = input.effective_body_json(body_json);

    let Some((mut source, _)) = build_local_openai_image_candidate_attempt_source(
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

    while let Some(attempt) = source.next_attempt().await? {
        if let Some(payload) = maybe_build_local_openai_image_decision_payload_for_candidate(
            state,
            parts,
            body_json,
            body_base64,
            trace_id,
            &input,
            attempt,
            spec,
        )
        .await?
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
    trace_id: &str,
    decision: &GatewayControlDecision,
    spec: LocalOpenAiImageSpec,
) -> Result<Vec<AiSyncAttempt>, GatewayError> {
    let spec_metadata = local_openai_image_spec_metadata(spec);
    let Some(input) = resolve_local_openai_image_decision_input(
        state,
        parts,
        body_json,
        body_base64,
        trace_id,
        decision,
    )
    .await?
    else {
        return Ok(Vec::new());
    };
    let body_json = input.effective_body_json(body_json);

    let Some((mut source, _)) = build_local_openai_image_candidate_attempt_source(
        state,
        trace_id,
        &input,
        body_json,
        spec_metadata.api_format,
        spec_metadata.decision_kind,
    )
    .await?
    else {
        return Ok(Vec::new());
    };

    let mut plans = Vec::new();
    while let Some(attempt) = source.next_attempt().await? {
        let Some(payload) = maybe_build_local_openai_image_decision_payload_for_candidate(
            state,
            parts,
            body_json,
            body_base64,
            trace_id,
            &input,
            attempt,
            spec,
        )
        .await?
        else {
            continue;
        };

        let provider_api_format = payload.provider_api_format.as_deref().unwrap_or_default();
        let built = if provider_api_format == "gemini:generate_content" {
            build_gemini_sync_plan_from_decision(parts, body_json, payload)
        } else {
            build_passthrough_sync_plan_from_decision(parts, payload)
        };
        match built {
            Ok(Some(value)) => plans.push(value),
            Ok(None) => {}
            Err(err) => {
                warn!(
                    trace_id = %trace_id,
                    decision_kind = spec_metadata.decision_kind,
                    error = ?err,
                    "gateway local openai image sync decision plan build failed"
                );
            }
        }
    }

    Ok(plans)
}

async fn build_local_stream_plan_and_reports(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    body_base64: Option<&str>,
    trace_id: &str,
    decision: &GatewayControlDecision,
    spec: LocalOpenAiImageSpec,
) -> Result<Vec<AiStreamAttempt>, GatewayError> {
    let spec_metadata = local_openai_image_spec_metadata(spec);
    let Some(input) = resolve_local_openai_image_decision_input(
        state,
        parts,
        body_json,
        body_base64,
        trace_id,
        decision,
    )
    .await?
    else {
        return Ok(Vec::new());
    };
    let body_json = input.effective_body_json(body_json);

    let Some((mut source, _)) = build_local_openai_image_candidate_attempt_source(
        state,
        trace_id,
        &input,
        body_json,
        spec_metadata.api_format,
        spec_metadata.decision_kind,
    )
    .await?
    else {
        return Ok(Vec::new());
    };

    let mut plans = Vec::new();
    while let Some(attempt) = source.next_attempt().await? {
        let Some(payload) = maybe_build_local_openai_image_decision_payload_for_candidate(
            state,
            parts,
            body_json,
            body_base64,
            trace_id,
            &input,
            attempt,
            spec,
        )
        .await?
        else {
            continue;
        };

        let provider_api_format = payload.provider_api_format.as_deref().unwrap_or_default();
        let built = if provider_api_format == "gemini:generate_content" {
            build_gemini_stream_plan_from_decision(parts, body_json, payload)
        } else {
            build_standard_stream_plan_from_decision(parts, body_json, payload, false)
        };
        match built {
            Ok(Some(value)) => plans.push(value),
            Ok(None) => {}
            Err(err) => {
                warn!(
                    trace_id = %trace_id,
                    decision_kind = spec_metadata.decision_kind,
                    error = ?err,
                    "gateway local openai image stream decision plan build failed"
                );
            }
        }
    }

    Ok(plans)
}
