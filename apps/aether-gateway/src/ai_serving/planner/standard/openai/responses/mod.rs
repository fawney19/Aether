use crate::ai_serving::planner::common::endpoint_config_forces_body_stream_field;
use crate::ai_serving::planner::plan_builders::{AiStreamAttempt, AiSyncAttempt};
use crate::ai_serving::planner::spec_metadata::local_openai_responses_spec_metadata;
use crate::ai_serving::planner::standard::codex::codex_model_capabilities_for_transport;
use crate::ai_serving::planner::standard::normalize::{
    build_local_openai_responses_request_body_with_codex_model_capabilities,
    build_local_openai_responses_request_body_with_codex_model_capabilities_for_websocket_continuation,
};
use crate::ai_serving::planner::standard::openai_responses_reasoning_replay_policy;
use crate::ai_serving::GatewayControlDecision;
use crate::orchestration::{
    codex_quota_breaker_blocks_candidate, log_codex_quota_breaker_check_failure,
    responses_websocket_adapter, ResponsesWebSocketAdapter,
};
use crate::{AiExecutionDecision, AppState, GatewayError};
use aether_runtime_state::RuntimeLockLease;
use std::collections::BTreeSet;

/// Releases a scheduler pool-key lease if WebSocket planning is cancelled
/// after candidate selection but before ownership reaches the turn lifecycle.
struct ResponsesWebSocketPlanningLeaseGuard {
    state: AppState,
    lease: Option<RuntimeLockLease>,
}

impl ResponsesWebSocketPlanningLeaseGuard {
    fn new(state: &AppState, lease: Option<&RuntimeLockLease>) -> Self {
        Self {
            state: state.clone(),
            lease: lease.cloned(),
        }
    }

    async fn release(mut self) {
        // Keep the lease armed across the await. If the owner task is aborted
        // or reaches its hard deadline while the runtime backend is stalled,
        // Drop can still hand cleanup to a detached owner.
        if release_responses_websocket_planning_lease(&self.state, self.lease.as_ref()).await {
            self.lease = None;
        }
    }

    fn disarm(&mut self) {
        self.lease = None;
    }
}

impl Drop for ResponsesWebSocketPlanningLeaseGuard {
    fn drop(&mut self) {
        let Some(lease) = self.lease.take() else {
            return;
        };
        let state = self.state.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = release_responses_websocket_planning_lease(&state, Some(&lease)).await;
            });
        }
    }
}

mod decision;
mod plans;

use self::decision::{
    build_local_openai_responses_candidate_attempt_source,
    maybe_build_local_openai_responses_decision_payload_for_candidate,
    maybe_build_local_openai_responses_decision_payload_for_candidate_with_websocket_mode,
    resolve_local_openai_responses_decision_input,
    resolve_local_openai_responses_decision_input_with_snapshot,
};
use self::plans::{
    build_local_stream_attempt_source, build_local_stream_plan_and_reports,
    build_local_sync_attempt_source, build_local_sync_plan_and_reports, resolve_stream_spec,
    resolve_sync_spec,
};

pub(crate) async fn build_local_openai_responses_sync_plan_and_reports_for_kind(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
) -> Result<Vec<AiSyncAttempt>, GatewayError> {
    let Some(spec) = resolve_sync_spec(plan_kind) else {
        return Ok(Vec::new());
    };

    build_local_sync_plan_and_reports(state, parts, trace_id, decision, body_json, spec).await
}

pub(crate) async fn build_local_openai_responses_stream_plan_and_reports_for_kind(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
) -> Result<Vec<AiStreamAttempt>, GatewayError> {
    let Some(spec) = resolve_stream_spec(plan_kind) else {
        return Ok(Vec::new());
    };

    build_local_stream_plan_and_reports(state, parts, trace_id, decision, body_json, spec).await
}

pub(crate) async fn build_local_openai_responses_sync_attempt_source_for_kind<'a>(
    state: &'a AppState,
    parts: &'a http::request::Parts,
    trace_id: &'a str,
    decision: &'a GatewayControlDecision,
    body_json: &'a serde_json::Value,
    plan_kind: &str,
) -> Result<
    Option<(
        impl crate::ai_serving::planner::LocalExecutionAttemptSource<AiSyncAttempt> + 'a,
        usize,
    )>,
    GatewayError,
> {
    let Some(spec) = resolve_sync_spec(plan_kind) else {
        return Ok(None);
    };

    build_local_sync_attempt_source(state, parts, trace_id, decision, body_json, spec).await
}

pub(crate) async fn build_local_openai_responses_stream_attempt_source_for_kind<'a>(
    state: &'a AppState,
    parts: &'a http::request::Parts,
    trace_id: &'a str,
    decision: &'a GatewayControlDecision,
    body_json: &'a serde_json::Value,
    plan_kind: &str,
) -> Result<
    Option<(
        impl crate::ai_serving::planner::LocalExecutionAttemptSource<AiStreamAttempt> + 'a,
        usize,
    )>,
    GatewayError,
> {
    let Some(spec) = resolve_stream_spec(plan_kind) else {
        return Ok(None);
    };

    build_local_stream_attempt_source(state, parts, trace_id, decision, body_json, spec).await
}

pub(crate) async fn maybe_build_sync_local_openai_responses_decision_payload(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
) -> Result<Option<AiExecutionDecision>, GatewayError> {
    let Some(spec) = resolve_sync_spec(plan_kind) else {
        return Ok(None);
    };

    let Some(input) = resolve_local_openai_responses_decision_input(
        state, parts, trace_id, decision, body_json, plan_kind,
    )
    .await?
    else {
        return Ok(None);
    };
    let body_json = input.effective_body_json(body_json);

    let (mut source, _) = build_local_openai_responses_candidate_attempt_source(
        state, trace_id, &input, body_json, spec,
    )
    .await?;

    while let Some(attempt) = source.next_attempt().await? {
        if let Some(payload) = maybe_build_local_openai_responses_decision_payload_for_candidate(
            state, parts, trace_id, body_json, &input, attempt, spec,
        )
        .await?
        {
            return Ok(Some(payload));
        }
    }

    Ok(None)
}

pub(crate) async fn maybe_build_stream_local_openai_responses_decision_payload(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
) -> Result<Option<AiExecutionDecision>, GatewayError> {
    let Some(spec) = resolve_stream_spec(plan_kind) else {
        return Ok(None);
    };

    let Some(input) = resolve_local_openai_responses_decision_input(
        state, parts, trace_id, decision, body_json, plan_kind,
    )
    .await?
    else {
        return Ok(None);
    };
    let body_json = input.effective_body_json(body_json);

    let (mut source, _) = build_local_openai_responses_candidate_attempt_source(
        state, trace_id, &input, body_json, spec,
    )
    .await?;

    while let Some(attempt) = source.next_attempt().await? {
        if let Some(payload) = maybe_build_local_openai_responses_decision_payload_for_candidate(
            state, parts, trace_id, body_json, &input, attempt, spec,
        )
        .await?
        {
            return Ok(Some(payload));
        }
    }

    Ok(None)
}

/// One eligible upstream plus the adapter that is allowed to speak to it.
///
/// The adapter is selected from the provider-scoped capability before the
/// decision leaves the planner. This prevents a public Responses socket from
/// choosing an arbitrary provider protocol after scheduling has completed.
pub(crate) struct ResponsesWebSocketDecision {
    pub(crate) execution: AiExecutionDecision,
    pub(crate) adapter: ResponsesWebSocketAdapter,
    pub(crate) normalization: ResponsesWebSocketBodyNormalization,
    /// Effective key auth after applying the endpoint API-format override.
    /// Protocol companions such as Codex Live must not infer this from a URL
    /// or from the presence of one particular generated header.
    pub(crate) effective_auth_type: String,
}

/// The scheduler identity a continuation is allowed to reuse.
///
/// A `previous_response_id` chain cannot move to another provider connection,
/// but it still has to pass the current scheduler runtime checks on every
/// turn. The planner uses this identity as a filter rather than selecting an
/// arbitrary eligible replacement.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResponsesWebSocketPinnedCandidate {
    provider_id: String,
    endpoint_id: String,
    key_id: String,
}

impl ResponsesWebSocketPinnedCandidate {
    pub(crate) fn new(provider_id: &str, endpoint_id: &str, key_id: &str) -> Option<Self> {
        Some(Self {
            provider_id: non_empty_decision_identity(Some(provider_id))?,
            endpoint_id: non_empty_decision_identity(Some(endpoint_id))?,
            key_id: non_empty_decision_identity(Some(key_id))?,
        })
    }

    pub(crate) fn from_decision(decision: &AiExecutionDecision) -> Option<Self> {
        Self::new(
            decision.provider_id.as_deref()?,
            decision.endpoint_id.as_deref()?,
            decision.key_id.as_deref()?,
        )
    }

    pub(crate) fn provider_id(&self) -> &str {
        self.provider_id.as_str()
    }

    pub(crate) fn endpoint_id(&self) -> &str {
        self.endpoint_id.as_str()
    }

    pub(crate) fn key_id(&self) -> &str {
        self.key_id.as_str()
    }

    fn matches(
        &self,
        candidate: &aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate,
    ) -> bool {
        candidate.provider_id == self.provider_id
            && candidate.endpoint_id == self.endpoint_id
            && candidate.key_id == self.key_id
    }
}

fn non_empty_decision_identity(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Everything needed to re-run provider-body normalization for the candidate a
/// socket is already bound to.
///
/// A continuation turn (`previous_response_id` on the bound upstream) cannot
/// re-enter the planner, because planning selects a candidate and a different
/// key would break the response chain. Without this, such turns reached the
/// provider with only their `model` rewritten — skipping model directives,
/// endpoint body rules, and the Codex body contract that turn 1 received.
///
/// This value holds cloned scalars and JSON only: no candidate, no pool key
/// lease, no `AppState`. It cannot influence selection.
#[derive(Debug, Clone)]
pub(crate) struct ResponsesWebSocketBodyNormalization {
    provider_type: String,
    provider_api_format: String,
    client_api_format: String,
    mapped_model: String,
    requested_model: String,
    upstream_is_stream: bool,
    force_body_stream_field: bool,
    body_rules: Option<serde_json::Value>,
    request_headers: http::HeaderMap,
    codex_model_capabilities: Option<crate::ai_serving::CodexResponsesModelCapabilities>,
    reasoning_replay_policy: crate::ai_serving::OpenAiResponsesReasoningReplayPolicy,
    model_directive_patch: Option<serde_json::Value>,
}

impl ResponsesWebSocketBodyNormalization {
    /// Builds a normalizer for a plain `openai:responses` upstream with no
    /// endpoint body rules, directives or Codex capabilities, so relay tests can
    /// construct a bound connection without standing up a provider snapshot.
    #[cfg(test)]
    pub(crate) fn for_tests(mapped_model: &str) -> Self {
        Self {
            provider_type: "openai".to_string(),
            provider_api_format: "openai:responses".to_string(),
            client_api_format: "openai:responses".to_string(),
            mapped_model: mapped_model.to_string(),
            requested_model: mapped_model.to_string(),
            upstream_is_stream: true,
            force_body_stream_field: false,
            body_rules: None,
            request_headers: http::HeaderMap::new(),
            codex_model_capabilities: None,
            reasoning_replay_policy:
                crate::ai_serving::OpenAiResponsesReasoningReplayPolicy::OpenAiItemIds,
            model_directive_patch: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_provider_type_for_tests(mut self, provider_type: &str) -> Self {
        self.provider_type = provider_type.to_string();
        self
    }

    #[cfg(test)]
    pub(crate) fn with_body_rules_for_tests(mut self, body_rules: serde_json::Value) -> Self {
        self.body_rules = Some(body_rules);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_reasoning_replay_policy_for_tests(
        mut self,
        reasoning_replay_policy: crate::ai_serving::OpenAiResponsesReasoningReplayPolicy,
    ) -> Self {
        self.reasoning_replay_policy = reasoning_replay_policy;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_model_directive_patch_for_tests(mut self, patch: serde_json::Value) -> Self {
        self.model_directive_patch = Some(patch);
        self
    }

    pub(crate) fn uses_codex_responses_lite(&self) -> bool {
        if !self.provider_type.trim().eq_ignore_ascii_case("codex")
            || !crate::ai_serving::is_openai_responses_family_format(
                self.provider_api_format.as_str(),
            )
        {
            return false;
        }
        self.codex_model_capabilities
            .clone()
            .unwrap_or_else(|| {
                crate::ai_serving::resolve_codex_responses_model_capabilities(
                    self.mapped_model.as_str(),
                    self.requested_model.as_str(),
                    None,
                )
            })
            .use_responses_lite
    }

    pub(crate) fn reasoning_replay_policy(
        &self,
    ) -> crate::ai_serving::OpenAiResponsesReasoningReplayPolicy {
        self.reasoning_replay_policy
    }

    /// Returns whether an enabled endpoint body rule that applies to this
    /// request owns the final value of a non-lineage WebSocket framing field.
    ///
    /// Codex's HTTP-shaped normalization intentionally removes or rewrites a
    /// few WebSocket-only fields. The framing layer may restore a value from
    /// the raw client event only when an administrator rule did not handle
    /// that path; otherwise the restore would silently undo the endpoint
    /// policy after all request finalization had completed. Opaque lineage
    /// (`previous_response_id`) is deliberately excluded by the framing layer:
    /// its final value must remain the authenticated client value.
    pub(crate) fn body_rules_handle_websocket_field(
        &self,
        client_event: &serde_json::Value,
        field: &str,
    ) -> bool {
        let Some(mut body_before_rules) =
            crate::ai_serving::build_local_openai_responses_request_body_with_model_directives(
                client_event,
                self.mapped_model.as_str(),
                self.upstream_is_stream,
                false,
            )
        else {
            // Normalization will reject the same malformed event. Keep the
            // framing pass fail closed if this method is ever called alone.
            return true;
        };
        crate::ai_serving::transport::rules::apply_local_body_rules_with_request_headers_and_track_path(
            &mut body_before_rules,
            self.body_rules.as_ref(),
            Some(client_event),
            Some(&self.request_headers),
            field,
        )
        .unwrap_or(true)
    }

    pub(crate) fn has_same_responses_lite_static_contract(&self, other: &Self) -> bool {
        self.provider_type
            .trim()
            .eq_ignore_ascii_case(other.provider_type.trim())
            && crate::ai_serving::api_format_alias_matches(
                self.provider_api_format.as_str(),
                other.provider_api_format.as_str(),
            )
            && self.mapped_model == other.mapped_model
            && self.requested_model == other.requested_model
            && self.body_rules == other.body_rules
            && self.codex_model_capabilities == other.codex_model_capabilities
            && self.model_directive_patch == other.model_directive_patch
            && self.uses_codex_responses_lite() == other.uses_codex_responses_lite()
    }

    /// Produces a versioned digest of the complete body-normalization
    /// contract. A continuation registry stores only this digest so a new
    /// socket can fail closed when endpoint rules, model capabilities or
    /// header-dependent normalization has changed, without persisting request
    /// headers or other sensitive configuration.
    pub(crate) fn continuation_fingerprint(&self) -> [u8; 32] {
        use sha2::Digest as _;

        let mut digest = sha2::Sha256::new();
        digest.update(b"aether-responses-websocket-normalization-v1");
        update_normalization_string_digest(&mut digest, self.provider_type.as_str());
        update_normalization_string_digest(&mut digest, self.provider_api_format.as_str());
        update_normalization_string_digest(&mut digest, self.client_api_format.as_str());
        update_normalization_string_digest(&mut digest, self.mapped_model.as_str());
        update_normalization_string_digest(&mut digest, self.requested_model.as_str());
        digest.update([
            u8::from(self.upstream_is_stream),
            u8::from(self.force_body_stream_field),
        ]);
        update_normalization_optional_json_digest(&mut digest, self.body_rules.as_ref());
        update_normalization_body_rule_headers_digest(
            &mut digest,
            &self.request_headers,
            self.body_rules.as_ref(),
        );
        update_normalization_codex_capabilities_digest(
            &mut digest,
            self.codex_model_capabilities.as_ref(),
        );
        digest.update([match self.reasoning_replay_policy {
            crate::ai_serving::OpenAiResponsesReasoningReplayPolicy::OpenAiItemIds => 0,
            crate::ai_serving::OpenAiResponsesReasoningReplayPolicy::DeepSeekOpaque => 1,
        }]);
        update_normalization_optional_json_digest(&mut digest, self.model_directive_patch.as_ref());
        digest.finalize().into()
    }

    /// Applies the same body transformations the planner applied on the turn
    /// that bound this upstream.
    ///
    /// Mirrors the same-format branch of
    /// `resolve_local_openai_responses_candidate_payload_parts`. The
    /// cross-format, Kiro, Windsurf and Antigravity branches are unreachable
    /// here: the WebSocket planner only returns candidates whose provider API
    /// format is `openai:responses`.
    ///
    /// Returns `None` when normalization fails. The WebSocket caller rejects
    /// that turn rather than sending an unnormalized event that bypasses body
    /// rules or replays a Responses Lite static prefix.
    pub(crate) fn normalize_response_create(
        &self,
        client_event: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        use crate::ai_serving::planner::common::{
            enforce_provider_body_stream_policy, request_requires_body_stream_field,
        };

        let source_model = client_event
            .get("model")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(self.requested_model.as_str());
        // The first response.create on a socket is a normal Responses request.
        // Only a non-empty previous_response_id denotes a continuation whose
        // stored history already contains the synthetic Responses Lite
        // tools/instructions prefix. Keep this discriminator explicit instead
        // of applying continuation edits to every socket turn.
        let websocket_continuation = client_event
            .get("previous_response_id")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| !value.trim().is_empty());
        let require_body_stream_field =
            request_requires_body_stream_field(client_event, self.force_body_stream_field);
        let mut body = if websocket_continuation {
            build_local_openai_responses_request_body_with_codex_model_capabilities_for_websocket_continuation(
                client_event,
                &self.mapped_model,
                self.upstream_is_stream,
                self.force_body_stream_field,
                self.provider_type.as_str(),
                self.provider_api_format.as_str(),
                self.body_rules.as_ref(),
                &self.request_headers,
                self.codex_model_capabilities.as_ref(),
                false,
            )
        } else {
            build_local_openai_responses_request_body_with_codex_model_capabilities(
                client_event,
                &self.mapped_model,
                self.upstream_is_stream,
                self.force_body_stream_field,
                self.provider_type.as_str(),
                self.provider_api_format.as_str(),
                self.body_rules.as_ref(),
                &self.request_headers,
                self.codex_model_capabilities.as_ref(),
                false,
            )
        }?;
        if let Some(patch) = self.model_directive_patch.as_ref() {
            crate::ai_serving::apply_model_directive_mapping_patch(&mut body, patch);
            // The patch is a deep merge and may reintroduce `stream`.
            enforce_provider_body_stream_policy(
                &mut body,
                self.provider_api_format.as_str(),
                self.upstream_is_stream,
                require_body_stream_field,
            );
        }
        let finalization = crate::ai_serving::OpenAiProviderRequestFinalization {
            source_api_format: self.client_api_format.as_str(),
            provider_api_format: self.provider_api_format.as_str(),
            provider_type: self.provider_type.as_str(),
            provider_model: self.mapped_model.as_str(),
            source_model,
            body_rules: self.body_rules.as_ref(),
            upstream_is_stream: self.upstream_is_stream,
            require_body_stream_field,
        };
        let finalized = if websocket_continuation {
            crate::ai_serving::finalize_openai_provider_request_with_codex_model_capabilities_and_reasoning_replay_policy_for_websocket_continuation(
                &mut body,
                finalization,
                self.codex_model_capabilities.as_ref(),
                self.reasoning_replay_policy,
            )
        } else {
            crate::ai_serving::finalize_openai_provider_request_with_codex_model_capabilities_and_reasoning_replay_policy(
                &mut body,
                finalization,
                self.codex_model_capabilities.as_ref(),
                self.reasoning_replay_policy,
            )
        };
        finalized.ok()?;
        Some(body)
    }
}

fn update_normalization_bytes_digest(digest: &mut sha2::Sha256, value: &[u8]) {
    use sha2::Digest as _;

    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

fn update_normalization_string_digest(digest: &mut sha2::Sha256, value: &str) {
    update_normalization_bytes_digest(digest, value.as_bytes());
}

fn update_normalization_optional_string_digest(digest: &mut sha2::Sha256, value: Option<&str>) {
    use sha2::Digest as _;

    match value {
        Some(value) => {
            digest.update([1]);
            update_normalization_string_digest(digest, value);
        }
        None => digest.update([0]),
    }
}

fn update_normalization_string_vec_digest(digest: &mut sha2::Sha256, values: &[String]) {
    use sha2::Digest as _;

    digest.update((values.len() as u64).to_be_bytes());
    for value in values {
        update_normalization_string_digest(digest, value);
    }
}

fn update_normalization_optional_json_digest(
    digest: &mut sha2::Sha256,
    value: Option<&serde_json::Value>,
) {
    use sha2::Digest as _;

    match value {
        Some(value) => {
            digest.update([1]);
            update_normalization_json_digest(digest, value);
        }
        None => digest.update([0]),
    }
}

fn update_normalization_json_digest(digest: &mut sha2::Sha256, value: &serde_json::Value) {
    use serde_json::Value;
    use sha2::Digest as _;

    match value {
        Value::Null => digest.update(b"n"),
        Value::Bool(value) => digest.update(if *value { b"t" } else { b"f" }),
        Value::Number(value) => {
            digest.update(b"d");
            update_normalization_string_digest(digest, value.to_string().as_str());
        }
        Value::String(value) => {
            digest.update(b"s");
            update_normalization_string_digest(digest, value);
        }
        Value::Array(values) => {
            digest.update(b"[");
            digest.update((values.len() as u64).to_be_bytes());
            for value in values {
                update_normalization_json_digest(digest, value);
            }
            digest.update(b"]");
        }
        Value::Object(values) => {
            digest.update(b"{");
            digest.update((values.len() as u64).to_be_bytes());
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            for key in keys {
                update_normalization_string_digest(digest, key);
                update_normalization_json_digest(digest, &values[key]);
            }
            digest.update(b"}");
        }
    }
}

fn update_normalization_body_rule_headers_digest(
    digest: &mut sha2::Sha256,
    headers: &http::HeaderMap,
    body_rules: Option<&serde_json::Value>,
) {
    use sha2::Digest as _;

    let dependencies =
        crate::ai_serving::transport::rules::body_rules_request_header_dependencies(body_rules);
    digest.update((dependencies.len() as u64).to_be_bytes());
    for name in dependencies {
        update_normalization_string_digest(digest, name.as_str());
        let value = headers
            .get(name.as_str())
            .and_then(|value| value.to_str().ok())
            .map(str::trim);
        update_normalization_optional_string_digest(digest, value);
    }
}

fn update_normalization_codex_capabilities_digest(
    digest: &mut sha2::Sha256,
    capabilities: Option<&crate::ai_serving::CodexResponsesModelCapabilities>,
) {
    use sha2::Digest as _;

    let Some(capabilities) = capabilities else {
        digest.update([0]);
        return;
    };
    digest.update([1]);
    digest.update([
        u8::from(capabilities.use_responses_lite),
        u8::from(capabilities.supports_reasoning_summary_parameter),
        u8::from(capabilities.supports_parallel_tool_calls),
        u8::from(capabilities.support_verbosity),
    ]);
    update_normalization_optional_string_digest(
        digest,
        capabilities.default_reasoning_effort.as_deref(),
    );
    update_normalization_optional_string_digest(
        digest,
        capabilities.default_reasoning_summary.as_deref(),
    );
    update_normalization_string_vec_digest(digest, &capabilities.supported_reasoning_efforts);
    update_normalization_optional_string_digest(digest, capabilities.default_verbosity.as_deref());
    update_normalization_string_vec_digest(digest, &capabilities.supported_service_tiers);
}

/// Builds one upstream decision for a Responses WebSocket turn. The session
/// reuses this decision for same-model turns and invokes the planner again when
/// a later `response.create` changes the public model.
pub(crate) async fn maybe_build_responses_websocket_decision(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    auth_snapshot: Option<&crate::ai_serving::GatewayAuthApiKeySnapshot>,
    body_json: &serde_json::Value,
    excluded_key_ids: Option<&BTreeSet<String>>,
    excluded_codex_account_ids: Option<&BTreeSet<String>>,
    pinned_candidate: Option<&ResponsesWebSocketPinnedCandidate>,
) -> Result<Option<ResponsesWebSocketDecision>, GatewayError> {
    let Some(spec) = resolve_stream_spec(crate::ai_serving::OPENAI_RESPONSES_STREAM_PLAN_KIND)
    else {
        return Ok(None);
    };
    let Some(input) = resolve_local_openai_responses_decision_input_with_snapshot(
        state,
        parts,
        trace_id,
        decision,
        body_json,
        spec.decision_kind,
        auth_snapshot,
    )
    .await?
    else {
        return Ok(None);
    };
    // The continuation discriminator belongs to the WebSocket protocol, not
    // provider body rules/redaction. Capture it before the planner creates its
    // effective body so a rule cannot accidentally turn a valid chain into a
    // first-turn Lite normalization pass.
    let websocket_continuation = body_json
        .get("previous_response_id")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|value| !value.trim().is_empty());
    let body_json = input.effective_body_json(body_json);
    let (mut source, _) = build_local_openai_responses_candidate_attempt_source(
        state, trace_id, &input, body_json, spec,
    )
    .await?;

    while let Some(attempt) = source.next_attempt().await? {
        // `next_attempt` may return with a distributed pool-key lease. Arm a
        // guard before the first await so owner-task timeout/cancellation
        // cannot strand that lease until its server-side TTL expires.
        let mut planning_lease = ResponsesWebSocketPlanningLeaseGuard::new(
            state,
            attempt.eligible.orchestration.pool_key_lease.as_ref(),
        );
        if pinned_candidate.is_some_and(|pinned| !pinned.matches(&attempt.eligible.candidate)) {
            planning_lease.release().await;
            continue;
        }
        if excluded_key_ids
            .is_some_and(|key_ids| key_ids.contains(attempt.eligible.candidate.key_id.as_str()))
        {
            planning_lease.release().await;
            continue;
        }
        let Some(adapter) = responses_websocket_adapter(
            &attempt.eligible.transport.provider.provider_type,
            attempt.eligible.transport.provider.config.as_ref(),
        ) else {
            planning_lease.release().await;
            continue;
        };
        // Captured before `attempt` is consumed so a later continuation turn can
        // reproduce this candidate's body normalization without re-planning.
        let transport = std::sync::Arc::clone(&attempt.eligible.transport);
        let effective_auth_type =
            aether_provider_transport::auth::resolve_local_auth_type_for_transport_format(
                transport.as_ref(),
            );
        let candidate_provider_api_format = attempt.eligible.provider_api_format.clone();
        let payload = match maybe_build_local_openai_responses_decision_payload_for_candidate_with_websocket_mode(
            state,
            parts,
            trace_id,
            body_json,
            &input,
            attempt,
            spec,
            websocket_continuation,
        )
        .await
        {
            Ok(Some(payload)) => payload,
            Ok(None) => {
                planning_lease.release().await;
                continue;
            }
            Err(error) => {
                planning_lease.release().await;
                return Err(error);
            }
        };
        if payload
            .provider_type
            .as_deref()
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("codex"))
            && crate::orchestration::codex_account_id_from_headers(
                &payload.provider_request_headers,
            )
            .is_some_and(|account_id| {
                excluded_codex_account_ids
                    .is_some_and(|account_ids| account_ids.contains(account_id))
            })
        {
            planning_lease.release().await;
            continue;
        }
        match codex_quota_breaker_blocks_candidate(
            state,
            payload.provider_type.as_deref(),
            payload.key_id.as_deref(),
            &payload.provider_request_headers,
        )
        .await
        {
            Ok(true) => {
                planning_lease.release().await;
                continue;
            }
            Ok(false) => {}
            Err(error) => log_codex_quota_breaker_check_failure(&error),
        }
        if payload
            .provider_type
            .as_deref()
            .is_some_and(|value| adapter.supports_provider_type(value))
            && payload.provider_api_format.as_deref().is_some_and(|value| {
                crate::ai_serving::normalize_api_format_alias(value) == "openai:responses"
            })
        {
            let mapped_model = payload.mapped_model.clone().unwrap_or_default();
            let source_model = body_json
                .get("model")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(input.requested_model.as_str());
            let normalization = ResponsesWebSocketBodyNormalization {
                provider_type: transport.provider.provider_type.clone(),
                provider_api_format: candidate_provider_api_format.clone(),
                client_api_format: local_openai_responses_spec_metadata(spec)
                    .api_format
                    .to_string(),
                requested_model: input.requested_model.clone(),
                upstream_is_stream: payload.upstream_is_stream,
                force_body_stream_field: endpoint_config_forces_body_stream_field(
                    transport.endpoint.config.as_ref(),
                ),
                body_rules: transport.endpoint.body_rules.clone(),
                request_headers: input.effective_headers(&parts.headers).clone(),
                codex_model_capabilities: codex_model_capabilities_for_transport(
                    &transport,
                    candidate_provider_api_format.as_str(),
                    mapped_model.as_str(),
                    source_model,
                ),
                reasoning_replay_policy: openai_responses_reasoning_replay_policy(
                    transport.provider.provider_type.as_str(),
                    transport.endpoint.base_url.as_str(),
                    mapped_model.as_str(),
                ),
                model_directive_patch: input
                    .model_directive_policy
                    .resolve_reasoning(
                        candidate_provider_api_format.as_str(),
                        Some(&input.requested_model),
                    )
                    .mapping_patch_for_mapped_model(mapped_model.as_str())
                    .ok()
                    .flatten(),
                mapped_model,
            };
            let decision = ResponsesWebSocketDecision {
                execution: payload,
                adapter,
                normalization,
                effective_auth_type,
            };
            // The decision report context now carries the lease identity. The
            // WebSocket ownership layer takes over before any further await.
            planning_lease.disarm();
            return Ok(Some(decision));
        }
        planning_lease.release().await;
    }

    Ok(None)
}

async fn release_responses_websocket_planning_lease(
    state: &AppState,
    lease: Option<&RuntimeLockLease>,
) -> bool {
    let Some(lease) = lease else {
        return true;
    };
    match crate::handlers::shared::provider_pool::release_admin_provider_pool_key_lease(
        state.runtime_state.as_ref(),
        lease,
    )
    .await
    {
        Ok(_) => true,
        Err(error) => {
            tracing::warn!(
                error = ?error,
                "gateway Responses WebSocket planner failed to release an unused pool key lease"
            );
            false
        }
    }
}

#[cfg(test)]
mod continuation_fingerprint_tests {
    use http::HeaderValue;
    use serde_json::json;

    use super::ResponsesWebSocketBodyNormalization;
    use crate::ai_serving::OpenAiResponsesReasoningReplayPolicy;

    #[test]
    fn normalization_fingerprint_is_stable_for_json_object_key_order() {
        let first = ResponsesWebSocketBodyNormalization::for_tests("provider-model")
            .with_model_directive_patch_for_tests(json!({"reasoning": {"effort": "high"}, "x": 1}));
        let second = ResponsesWebSocketBodyNormalization::for_tests("provider-model")
            .with_model_directive_patch_for_tests(json!({"x": 1, "reasoning": {"effort": "high"}}));
        assert_eq!(
            first.continuation_fingerprint(),
            second.continuation_fingerprint()
        );
    }

    #[test]
    fn normalization_fingerprint_changes_with_effective_contract() {
        let base = ResponsesWebSocketBodyNormalization::for_tests("provider-model");
        let changed_policy = base.clone().with_reasoning_replay_policy_for_tests(
            OpenAiResponsesReasoningReplayPolicy::DeepSeekOpaque,
        );
        assert_ne!(
            base.continuation_fingerprint(),
            changed_policy.continuation_fingerprint()
        );

        let changed_patch = base
            .clone()
            .with_model_directive_patch_for_tests(json!({"reasoning": {"effort": "low"}}));
        assert_ne!(
            base.continuation_fingerprint(),
            changed_patch.continuation_fingerprint()
        );
    }

    #[test]
    fn normalization_fingerprint_ignores_unrelated_volatile_request_headers() {
        let body_rules = json!([{
            "action": "set",
            "path": "store",
            "value": false,
            "condition": {
                "source": "request_headers",
                "path": "x-contract",
                "op": "eq",
                "value": "enabled"
            }
        }]);
        let mut first = ResponsesWebSocketBodyNormalization::for_tests("provider-model")
            .with_body_rules_for_tests(body_rules);
        first
            .request_headers
            .insert("x-contract", HeaderValue::from_static("enabled"));
        first
            .request_headers
            .insert("x-request-id", HeaderValue::from_static("request-1"));
        first
            .request_headers
            .insert("cf-ray", HeaderValue::from_static("edge-1"));
        let mut second = first.clone();
        second
            .request_headers
            .insert("x-request-id", HeaderValue::from_static("request-2"));
        second
            .request_headers
            .insert("cf-ray", HeaderValue::from_static("edge-2"));

        assert_eq!(
            first.continuation_fingerprint(),
            second.continuation_fingerprint(),
            "headers that no body-rule condition reads must not invalidate a persisted continuation"
        );
    }

    #[test]
    fn normalization_fingerprint_tracks_headers_used_by_body_rule_conditions() {
        let body_rules = json!([{
            "action": "set",
            "path": "store",
            "value": false,
            "condition": {
                "source": "request_headers",
                "path": "X-Contract",
                "op": "eq",
                "value": "enabled"
            }
        }]);
        let mut first = ResponsesWebSocketBodyNormalization::for_tests("provider-model")
            .with_body_rules_for_tests(body_rules.clone());
        first
            .request_headers
            .insert("x-contract", HeaderValue::from_static("enabled"));
        let mut second = ResponsesWebSocketBodyNormalization::for_tests("provider-model")
            .with_body_rules_for_tests(body_rules);
        second
            .request_headers
            .insert("x-contract", HeaderValue::from_static("disabled"));

        assert_ne!(
            first.continuation_fingerprint(),
            second.continuation_fingerprint(),
            "a header that controls an effective body-rule condition remains part of the contract"
        );
    }
}
