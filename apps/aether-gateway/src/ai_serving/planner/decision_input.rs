use std::collections::BTreeMap;

use aether_ai_serving::{run_ai_authenticated_decision_input, AiAuthenticatedDecisionInputPort};
use aether_routing_core::{
    rank_vector_for_candidate, CandidateKind, ResolvedRoutingPolicy, RoutingCandidateFacts,
    RoutingCandidateTrace, RoutingDecisionTrace, RoutingPoolExpansionTrace, RoutingRulePhase,
};
use aether_scheduler_core::ClientSessionAffinity;
use async_trait::async_trait;
use http::StatusCode;
use http::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{json, Value};
use tracing::warn;

use crate::ai_serving::planner::common::extract_standard_requested_model;
use crate::ai_serving::{ExecutionRuntimeAuthContext, GatewayAuthApiKeySnapshot, PlannerAppState};
use crate::client_session_affinity::client_session_affinity_from_request;
use crate::clock::current_unix_secs;
use crate::routing::{
    apply_routing_mutation_plan, build_routing_trace_seed, resolve_gateway_routing_policy,
    select_gateway_routing_group, GatewayRoutingPolicyInput, GatewayRoutingSelectionError,
    GatewayRoutingSelectionInput, ROUTING_GROUP_HEADER,
};
use crate::{AiExecutionDecision, AppState, GatewayError};

#[derive(Debug, Clone)]
pub(crate) struct ResolvedLocalDecisionAuthInput {
    pub(crate) auth_context: ExecutionRuntimeAuthContext,
    pub(crate) auth_snapshot: GatewayAuthApiKeySnapshot,
    pub(crate) required_capabilities: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub(crate) struct LocalRequestedModelDecisionInput {
    pub(crate) auth_context: ExecutionRuntimeAuthContext,
    pub(crate) requested_model: String,
    pub(crate) auth_snapshot: GatewayAuthApiKeySnapshot,
    pub(crate) required_capabilities: Option<serde_json::Value>,
    pub(crate) request_auth_channel: Option<String>,
    pub(crate) client_session_affinity: Option<ClientSessionAffinity>,
    pub(crate) routing_policy: Option<ResolvedRoutingPolicy>,
    pub(crate) routing_trace_seed: Option<RoutingDecisionTrace>,
    pub(crate) routing_context: Option<LocalRoutingRequestContext>,
}

#[derive(Debug, Clone)]
pub(crate) struct LocalAuthenticatedDecisionInput {
    pub(crate) auth_context: ExecutionRuntimeAuthContext,
    pub(crate) auth_snapshot: GatewayAuthApiKeySnapshot,
    pub(crate) required_capabilities: Option<serde_json::Value>,
    pub(crate) client_session_affinity: Option<ClientSessionAffinity>,
}

#[derive(Debug, Clone)]
pub(crate) struct LocalRoutingRequestContext {
    pub(crate) group_id: Option<String>,
    pub(crate) group_version: Option<i64>,
    pub(crate) group_config_json: Value,
    pub(crate) selection_source: String,
    pub(crate) client_api_format: String,
    pub(crate) effective_body_json: Value,
    pub(crate) effective_headers: HeaderMap,
}

impl LocalRequestedModelDecisionInput {
    pub(crate) fn effective_body_json<'a>(&'a self, fallback: &'a Value) -> &'a Value {
        self.routing_context
            .as_ref()
            .map(|context| &context.effective_body_json)
            .unwrap_or(fallback)
    }

    pub(crate) fn effective_headers<'a>(&'a self, fallback: &'a HeaderMap) -> &'a HeaderMap {
        self.routing_context
            .as_ref()
            .map(|context| &context.effective_headers)
            .unwrap_or(fallback)
    }
}

pub(crate) fn apply_provider_request_routing_policy_to_decision(
    input: &LocalRequestedModelDecisionInput,
    decision: &mut AiExecutionDecision,
) -> Result<(), GatewayError> {
    let Some(context) = input.routing_context.as_ref() else {
        return Ok(());
    };
    let provider_api_format = decision
        .provider_api_format
        .as_deref()
        .unwrap_or(context.client_api_format.as_str());
    let resolved_model = decision
        .mapped_model
        .as_deref()
        .or(decision.model_name.as_deref())
        .unwrap_or(input.requested_model.as_str());
    let original_provider_request_body = decision.provider_request_body.clone();
    let mut provider_request_body = original_provider_request_body
        .clone()
        .unwrap_or(serde_json::Value::Null);
    let mut provider_headers = btree_headers_to_header_map(&decision.provider_request_headers)?;
    let provider_headers_json = headers_to_routing_value(&provider_headers);
    let policy = resolve_gateway_routing_policy(GatewayRoutingPolicyInput {
        group_id: context.group_id.as_deref(),
        group_version: context.group_version,
        group_config_json: &context.group_config_json,
        selection_source: context.selection_source.as_str(),
        requested_model: input.requested_model.as_str(),
        resolved_model,
        api_format: provider_api_format,
        user_id: Some(input.auth_context.user_id.as_str()),
        api_key_id: Some(input.auth_context.api_key_id.as_str()),
        headers: &provider_headers_json,
        body: &provider_request_body,
        phase: RoutingRulePhase::ProviderRequest,
    })?;
    ensure_report_context_routing_trace(input, decision, &policy);
    if policy.mutation_plan.is_empty() {
        return Ok(());
    }
    if original_provider_request_body.is_none() && !policy.mutation_plan.body_patch.is_empty() {
        return Err(GatewayError::Client {
            status: StatusCode::BAD_REQUEST,
            message: "routing provider_request body patch cannot be applied to a binary or empty upstream body".to_string(),
        });
    }
    apply_routing_mutation_plan(
        &mut provider_request_body,
        &mut provider_headers,
        &policy.mutation_plan,
    )?;
    decision.provider_request_headers = header_map_to_btree_headers(&provider_headers);
    if original_provider_request_body.is_some() {
        decision.provider_request_body = Some(provider_request_body);
    }
    update_report_context_provider_request_mutation(decision, &policy);
    Ok(())
}

struct GatewayAuthenticatedDecisionInputPort<'a> {
    state: PlannerAppState<'a>,
    now_unix_secs: u64,
}

#[async_trait]
impl AiAuthenticatedDecisionInputPort for GatewayAuthenticatedDecisionInputPort<'_> {
    type AuthContext = ExecutionRuntimeAuthContext;
    type AuthSnapshot = GatewayAuthApiKeySnapshot;
    type RequiredCapabilities = serde_json::Value;
    type ResolvedInput = ResolvedLocalDecisionAuthInput;
    type Error = GatewayError;

    async fn read_auth_snapshot(
        &self,
        auth_context: &Self::AuthContext,
    ) -> Result<Option<Self::AuthSnapshot>, Self::Error> {
        self.state
            .read_auth_api_key_snapshot(
                &auth_context.user_id,
                &auth_context.api_key_id,
                self.now_unix_secs,
            )
            .await
    }

    async fn resolve_required_capabilities(
        &self,
        auth_context: &Self::AuthContext,
        requested_model: Option<&str>,
        explicit_required_capabilities: Option<&Self::RequiredCapabilities>,
    ) -> Result<Option<Self::RequiredCapabilities>, Self::Error> {
        Ok(self
            .state
            .resolve_request_candidate_required_capabilities(
                &auth_context.user_id,
                &auth_context.api_key_id,
                requested_model,
                explicit_required_capabilities,
            )
            .await)
    }

    fn build_resolved_input(
        &self,
        auth_context: Self::AuthContext,
        auth_snapshot: Self::AuthSnapshot,
        required_capabilities: Option<Self::RequiredCapabilities>,
    ) -> Self::ResolvedInput {
        ResolvedLocalDecisionAuthInput {
            auth_context,
            auth_snapshot,
            required_capabilities,
        }
    }
}

pub(crate) fn build_local_requested_model_decision_input(
    resolved_input: ResolvedLocalDecisionAuthInput,
    requested_model: String,
) -> LocalRequestedModelDecisionInput {
    LocalRequestedModelDecisionInput {
        auth_context: resolved_input.auth_context,
        requested_model,
        auth_snapshot: resolved_input.auth_snapshot,
        required_capabilities: resolved_input.required_capabilities,
        request_auth_channel: None,
        client_session_affinity: None,
        routing_policy: None,
        routing_trace_seed: None,
        routing_context: None,
    }
}

pub(crate) async fn attach_routing_policy_to_local_requested_model_input(
    state: &AppState,
    parts: &http::request::Parts,
    input: &mut LocalRequestedModelDecisionInput,
    body_json: &Value,
    client_api_format: &str,
) -> Result<(), GatewayError> {
    let explicit_group = routing_header_value_str(&parts.headers, ROUTING_GROUP_HEADER);
    let selected_group = match state.routing_group_read_repository() {
        Some(repository) => {
            let user_group_ids = match state
                .list_user_groups_for_user(&input.auth_context.user_id)
                .await
            {
                Ok(groups) => groups.into_iter().map(|group| group.id).collect::<Vec<_>>(),
                Err(error) => {
                    warn!(
                        user_id = %input.auth_context.user_id,
                        error = ?error,
                        "gateway routing profile user group lookup failed"
                    );
                    Vec::new()
                }
            };
            let selection = select_gateway_routing_group(
                repository.as_ref(),
                GatewayRoutingSelectionInput {
                    explicit_group: explicit_group.as_deref(),
                    user_id: Some(input.auth_context.user_id.as_str()),
                    api_key_id: Some(input.auth_context.api_key_id.as_str()),
                    user_group_ids: &user_group_ids,
                },
            )
            .await
            .map_err(routing_selection_error)?;
            selection.group.map(|group| {
                (
                    Some(group.id),
                    Some(group.version),
                    group.config_json,
                    selection.source,
                )
            })
        }
        None => {
            if explicit_group
                .as_deref()
                .map(str::trim)
                .is_some_and(|value| !value.is_empty())
            {
                return Err(routing_selection_error(
                    GatewayRoutingSelectionError::NotFound(explicit_group.unwrap_or_default()),
                ));
            }
            None
        }
    };

    let Some((group_id, group_version, group_config_json, selection_source)) = selected_group
    else {
        input.client_session_affinity =
            client_session_affinity_from_request(&parts.headers, Some(body_json));
        input.routing_policy = None;
        input.routing_trace_seed = None;
        input.routing_context = None;
        return Ok(());
    };

    let headers_json = headers_to_routing_value(&parts.headers);
    let policy = resolve_gateway_routing_policy(GatewayRoutingPolicyInput {
        group_id: group_id.as_deref(),
        group_version,
        group_config_json: &group_config_json,
        selection_source: selection_source.as_str(),
        requested_model: input.requested_model.as_str(),
        resolved_model: input.requested_model.as_str(),
        api_format: client_api_format,
        user_id: Some(input.auth_context.user_id.as_str()),
        api_key_id: Some(input.auth_context.api_key_id.as_str()),
        headers: &headers_json,
        body: body_json,
        phase: RoutingRulePhase::ClientRequest,
    })?;
    let mut effective_body_json = body_json.clone();
    let mut effective_headers = parts.headers.clone();
    apply_routing_mutation_plan(
        &mut effective_body_json,
        &mut effective_headers,
        &policy.mutation_plan,
    )?;

    let mut requested_model_changed = false;
    if let Some(mut mutated_model) = extract_standard_requested_model(&effective_body_json) {
        mutated_model = mutated_model.trim().to_string();
        if !mutated_model.is_empty() && mutated_model != input.requested_model {
            input.requested_model = mutated_model;
            requested_model_changed = true;
        }
    }
    if requested_model_changed {
        input.required_capabilities = PlannerAppState::new(state)
            .resolve_request_candidate_required_capabilities(
                &input.auth_context.user_id,
                &input.auth_context.api_key_id,
                Some(input.requested_model.as_str()),
                input.required_capabilities.as_ref(),
            )
            .await;
    }

    let effective_headers_json = headers_to_routing_value(&effective_headers);
    input.client_session_affinity =
        client_session_affinity_from_request(&effective_headers, Some(&effective_body_json));
    let mut final_policy = resolve_gateway_routing_policy(GatewayRoutingPolicyInput {
        group_id: group_id.as_deref(),
        group_version,
        group_config_json: &group_config_json,
        selection_source: selection_source.as_str(),
        requested_model: input.requested_model.as_str(),
        resolved_model: input.requested_model.as_str(),
        api_format: client_api_format,
        user_id: Some(input.auth_context.user_id.as_str()),
        api_key_id: Some(input.auth_context.api_key_id.as_str()),
        headers: &effective_headers_json,
        body: &effective_body_json,
        phase: RoutingRulePhase::ClientRequest,
    })?;
    final_policy.mutation_plan = policy.mutation_plan.clone();
    input.routing_trace_seed = Some(build_routing_trace_seed(&final_policy, client_api_format));
    input.routing_policy = Some(final_policy);
    input.routing_context = Some(LocalRoutingRequestContext {
        group_id,
        group_version,
        group_config_json,
        selection_source,
        client_api_format: client_api_format.to_string(),
        effective_body_json,
        effective_headers,
    });
    Ok(())
}

pub(crate) fn build_local_authenticated_decision_input(
    resolved_input: ResolvedLocalDecisionAuthInput,
) -> LocalAuthenticatedDecisionInput {
    LocalAuthenticatedDecisionInput {
        auth_context: resolved_input.auth_context,
        auth_snapshot: resolved_input.auth_snapshot,
        required_capabilities: resolved_input.required_capabilities,
        client_session_affinity: None,
    }
}

pub(crate) async fn resolve_local_authenticated_decision_input(
    state: &AppState,
    auth_context: ExecutionRuntimeAuthContext,
    requested_model: Option<&str>,
    explicit_required_capabilities: Option<&serde_json::Value>,
) -> Result<Option<ResolvedLocalDecisionAuthInput>, GatewayError> {
    let port = GatewayAuthenticatedDecisionInputPort {
        state: PlannerAppState::new(state),
        now_unix_secs: current_unix_secs(),
    };

    run_ai_authenticated_decision_input(
        &port,
        auth_context,
        requested_model,
        explicit_required_capabilities,
    )
    .await
}

fn routing_selection_error(error: GatewayRoutingSelectionError) -> GatewayError {
    GatewayError::Client {
        status: StatusCode::FORBIDDEN,
        message: error.to_string(),
    }
}

fn headers_to_routing_value(headers: &http::HeaderMap) -> Value {
    let mut object = serde_json::Map::new();
    for (name, value) in headers {
        if let Ok(value) = value.to_str() {
            object.insert(name.as_str().to_ascii_lowercase(), json!(value));
        }
    }
    Value::Object(object)
}

fn routing_header_value_str(headers: &http::HeaderMap, key: &str) -> Option<String> {
    headers
        .get(key)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn btree_headers_to_header_map(
    headers: &BTreeMap<String, String>,
) -> Result<HeaderMap, GatewayError> {
    let mut output = HeaderMap::new();
    for (name, value) in headers {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|err| GatewayError::Client {
            status: StatusCode::BAD_REQUEST,
            message: format!("invalid provider request header name in routing mutation: {err}"),
        })?;
        let value = HeaderValue::from_str(value).map_err(|err| GatewayError::Client {
            status: StatusCode::BAD_REQUEST,
            message: format!("invalid provider request header value in routing mutation: {err}"),
        })?;
        output.insert(name, value);
    }
    Ok(output)
}

fn header_map_to_btree_headers(headers: &HeaderMap) -> BTreeMap<String, String> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_string(), value.to_string()))
        })
        .collect()
}

fn update_report_context_provider_request_mutation(
    decision: &mut AiExecutionDecision,
    policy: &ResolvedRoutingPolicy,
) {
    let Some(serde_json::Value::Object(object)) = decision.report_context.as_mut() else {
        return;
    };
    let body_paths = policy
        .mutation_plan
        .body_patch
        .iter()
        .map(|operation| operation.path().to_string())
        .collect::<Vec<_>>();
    let header_names = policy
        .mutation_plan
        .header_patch
        .iter()
        .map(|operation| operation.name().to_string())
        .collect::<Vec<_>>();
    let trace_patch_summary = serde_json::json!({
        "body_paths": body_paths,
        "header_names": header_names,
    });
    if let Some(serde_json::Value::Object(routing_trace)) = object.get_mut("routing_trace") {
        routing_trace.insert(
            "provider_request_patch_summary".to_string(),
            trace_patch_summary.clone(),
        );
    }
    object.insert(
        "provider_request_headers".to_string(),
        serde_json::json!(decision.provider_request_headers),
    );
    object.insert(
        "routing_provider_request_patch_summary".to_string(),
        serde_json::json!({
            "body_paths": trace_patch_summary["body_paths"].clone(),
            "header_names": trace_patch_summary["header_names"].clone(),
            "matched_rules": policy
                .matched_rules
                .iter()
                .map(|rule| rule.id.clone())
                .collect::<Vec<_>>()
        }),
    );
}

fn ensure_report_context_routing_trace(
    input: &LocalRequestedModelDecisionInput,
    decision: &mut AiExecutionDecision,
    policy: &ResolvedRoutingPolicy,
) {
    let Some(serde_json::Value::Object(object)) = decision.report_context.as_mut() else {
        return;
    };
    if object.get("routing_trace").is_some() {
        return;
    }

    let client_api_format = decision
        .client_api_format
        .as_deref()
        .or_else(|| {
            input
                .routing_context
                .as_ref()
                .map(|context| context.client_api_format.as_str())
        })
        .unwrap_or_default();
    let mut trace = input
        .routing_trace_seed
        .clone()
        .unwrap_or_else(|| build_routing_trace_seed(policy, client_api_format));

    let candidate_group_id = object
        .get("candidate_group_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let pool_key_index = object
        .get("pool_key_index")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok());
    let is_pool_expansion = candidate_group_id.is_some() && pool_key_index.is_some();
    let candidate_kind = if is_pool_expansion {
        CandidateKind::PoolGroup
    } else {
        CandidateKind::Provider
    };
    let provider_id = candidate_group_id
        .clone()
        .or_else(|| decision.provider_id.clone())
        .unwrap_or_default();
    let endpoint_id = decision.endpoint_id.clone().unwrap_or_default();
    let model_id = object
        .get("model_id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| decision.mapped_model.clone())
        .or_else(|| decision.model_name.clone())
        .unwrap_or_else(|| input.requested_model.clone());
    let key_id = decision.key_id.clone().filter(|_| !is_pool_expansion);
    let provider_priority = object
        .get("provider_priority")
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
        .unwrap_or_default();
    let key_priority = object
        .get("priority_slot")
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
        .unwrap_or_default();
    trace.global_candidates.push(RoutingCandidateTrace {
        candidate_kind,
        provider_id: provider_id.clone(),
        endpoint_id,
        model_id: model_id.clone(),
        key_id: key_id.clone(),
        ranking_vector: rank_vector_for_candidate(
            &policy.ranking_overlay,
            &RoutingCandidateFacts {
                candidate_kind,
                provider_id: provider_id.clone(),
                endpoint_id: decision.endpoint_id.clone().unwrap_or_default(),
                model_id,
                key_id,
                provider_priority,
                key_priority,
            },
        ),
        skip_reason: None,
        selected_order: object
            .get("candidate_index")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok()),
    });

    if is_pool_expansion {
        if let (Some(pool_group_id), Some(key_id)) = (candidate_group_id, decision.key_id.clone()) {
            trace.pool_expansion.push(RoutingPoolExpansionTrace {
                pool_group_id,
                key_id,
                pool_ranking_vector: Vec::new(),
                pool_skip_reason: None,
                selected_order: pool_key_index,
            });
        }
    }

    object.insert("routing_trace".to_string(), serde_json::json!(trace));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_auth_context() -> ExecutionRuntimeAuthContext {
        ExecutionRuntimeAuthContext {
            user_id: "user-1".to_string(),
            api_key_id: "api-key-1".to_string(),
            username: None,
            api_key_name: None,
            balance_remaining: None,
            access_allowed: true,
            api_key_is_standalone: false,
        }
    }

    fn sample_auth_snapshot() -> GatewayAuthApiKeySnapshot {
        GatewayAuthApiKeySnapshot {
            user_id: "user-1".to_string(),
            username: "alice".to_string(),
            email: None,
            user_role: "user".to_string(),
            user_auth_source: "local".to_string(),
            user_is_active: true,
            user_is_deleted: false,
            user_rate_limit: None,
            user_allowed_providers: None,
            user_allowed_api_formats: None,
            user_allowed_models: None,
            api_key_id: "api-key-1".to_string(),
            api_key_name: Some("default".to_string()),
            api_key_is_active: true,
            api_key_is_locked: false,
            api_key_is_standalone: false,
            api_key_rate_limit: None,
            api_key_concurrent_limit: None,
            api_key_expires_at_unix_secs: None,
            api_key_allowed_providers: None,
            api_key_allowed_api_formats: None,
            api_key_allowed_models: None,
            api_key_ip_rules: None,
            currently_usable: true,
        }
    }

    fn sample_decision_input() -> LocalRequestedModelDecisionInput {
        LocalRequestedModelDecisionInput {
            auth_context: sample_auth_context(),
            requested_model: "gpt-5".to_string(),
            auth_snapshot: sample_auth_snapshot(),
            required_capabilities: None,
            request_auth_channel: None,
            client_session_affinity: None,
            routing_policy: None,
            routing_trace_seed: None,
            routing_context: Some(LocalRoutingRequestContext {
                group_id: Some("group-1".to_string()),
                group_version: Some(3),
                selection_source: "explicit_header".to_string(),
                client_api_format: "openai:chat".to_string(),
                effective_body_json: json!({"model":"gpt-5"}),
                effective_headers: HeaderMap::new(),
                group_config_json: json!({
                    "allowed_models": ["gpt-5"],
                    "rules": [{
                        "id": "provider-patch",
                        "priority": 1,
                        "enabled": true,
                        "phase": "provider_request",
                        "conditions": {},
                        "actions": [
                            {
                                "type": "json_patch_body",
                                "patch": [{
                                    "op": "add",
                                    "path": "/metadata/routing",
                                    "value": "provider"
                                }]
                            },
                            {
                                "type": "patch_headers",
                                "patch": [{
                                    "op": "set",
                                    "name": "x-provider-route",
                                    "value": "provider"
                                }]
                            }
                        ]
                    }]
                }),
            }),
        }
    }

    fn sample_decision() -> AiExecutionDecision {
        AiExecutionDecision {
            action: "execution_runtime_sync_decision".to_string(),
            decision_kind: Some("openai_chat_sync".to_string()),
            execution_strategy: None,
            conversion_mode: None,
            request_id: Some("trace-1".to_string()),
            candidate_id: Some("candidate-1".to_string()),
            provider_name: Some("provider".to_string()),
            provider_id: Some("provider-1".to_string()),
            endpoint_id: Some("endpoint-1".to_string()),
            key_id: Some("key-1".to_string()),
            upstream_base_url: None,
            upstream_url: None,
            provider_request_method: None,
            auth_header: None,
            auth_value: None,
            provider_api_format: Some("openai:chat".to_string()),
            client_api_format: Some("openai:chat".to_string()),
            provider_contract: None,
            client_contract: None,
            model_name: Some("gpt-5".to_string()),
            mapped_model: Some("gpt-5".to_string()),
            prompt_cache_key: None,
            extra_headers: BTreeMap::new(),
            provider_request_headers: BTreeMap::from([(
                "content-type".to_string(),
                "application/json".to_string(),
            )]),
            provider_request_body: Some(json!({"model":"gpt-5","metadata":{}})),
            provider_request_body_base64: None,
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            request_gzip: None,
            proxy: None,
            transport_profile: None,
            timeouts: None,
            upstream_is_stream: false,
            report_kind: Some("local_sync_success".to_string()),
            report_context: Some(json!({
                "candidate_index": 0,
                "retry_index": 0,
                "model_id": "model-1"
            })),
            auth_context: Some(sample_auth_context()),
        }
    }

    fn set_provider_request_rules(input: &mut LocalRequestedModelDecisionInput, actions: Value) {
        let config = json!({
            "allowed_models": ["gpt-5"],
            "rules": [{
                "id": "provider-patch",
                "priority": 1,
                "enabled": true,
                "phase": "provider_request",
                "conditions": {},
                "actions": actions
            }]
        });
        input
            .routing_context
            .as_mut()
            .expect("sample input should include routing context")
            .group_config_json = config;
    }

    #[test]
    fn provider_request_routing_policy_mutates_decision_body_headers_and_report_context() {
        let input = sample_decision_input();
        let mut decision = sample_decision();

        apply_provider_request_routing_policy_to_decision(&input, &mut decision)
            .expect("provider routing mutation should apply");

        assert_eq!(
            decision.provider_request_body.as_ref().unwrap()["metadata"]["routing"],
            json!("provider")
        );
        assert_eq!(
            decision
                .provider_request_headers
                .get("x-provider-route")
                .map(String::as_str),
            Some("provider")
        );
        let report_context = decision.report_context.as_ref().unwrap();
        assert_eq!(
            report_context["routing_provider_request_patch_summary"]["matched_rules"],
            json!(["provider-patch"])
        );
        assert_eq!(
            report_context["routing_trace"]["provider_request_patch_summary"]["body_paths"],
            json!(["/metadata/routing"])
        );
        assert_eq!(
            report_context["routing_trace"]["global_candidates"][0]["provider_id"],
            json!("provider-1")
        );
    }

    #[test]
    fn provider_request_routing_policy_rejects_body_patch_without_json_body() {
        let input = sample_decision_input();
        let mut decision = sample_decision();
        decision.provider_request_body = None;
        decision.provider_request_body_base64 = Some("AA==".to_string());

        let error = apply_provider_request_routing_policy_to_decision(&input, &mut decision)
            .expect_err("provider body patch should reject binary upstream bodies");

        match error {
            GatewayError::Client { status, message } => {
                assert_eq!(status, StatusCode::BAD_REQUEST);
                assert!(message.contains("binary or empty upstream body"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
        assert!(
            decision
                .report_context
                .as_ref()
                .and_then(|context| context.get("routing_trace"))
                .is_some(),
            "failed provider_request mutation should still seed routing trace"
        );
    }

    #[test]
    fn provider_request_routing_policy_allows_header_patch_without_json_body() {
        let mut input = sample_decision_input();
        set_provider_request_rules(
            &mut input,
            json!([{
                "type": "patch_headers",
                "patch": [{
                    "op": "set",
                    "name": "x-provider-route",
                    "value": "header-only"
                }]
            }]),
        );
        let mut decision = sample_decision();
        decision.provider_request_body = None;
        decision.provider_request_body_base64 = Some("AA==".to_string());

        apply_provider_request_routing_policy_to_decision(&input, &mut decision)
            .expect("header-only provider routing mutation should apply without JSON body");

        assert_eq!(decision.provider_request_body, None);
        assert_eq!(
            decision
                .provider_request_headers
                .get("x-provider-route")
                .map(String::as_str),
            Some("header-only")
        );
        assert_eq!(
            decision.report_context.as_ref().unwrap()["routing_trace"]
                ["provider_request_patch_summary"]["header_names"],
            json!(["x-provider-route"])
        );
    }

    #[test]
    fn provider_request_routing_trace_records_pool_expansion_candidate() {
        let input = sample_decision_input();
        let mut decision = sample_decision();
        decision.report_context = Some(json!({
            "candidate_index": 2,
            "retry_index": 2,
            "model_id": "model-1",
            "candidate_group_id": "pool-group-1",
            "pool_key_index": 1,
            "provider_priority": 7,
            "priority_slot": 3
        }));

        apply_provider_request_routing_policy_to_decision(&input, &mut decision)
            .expect("provider routing mutation should seed pool trace");

        let routing_trace = &decision.report_context.as_ref().unwrap()["routing_trace"];
        assert_eq!(
            routing_trace["global_candidates"][0]["candidate_kind"],
            json!("pool_group")
        );
        assert_eq!(
            routing_trace["global_candidates"][0]["provider_id"],
            json!("pool-group-1")
        );
        assert_eq!(routing_trace["global_candidates"][0]["key_id"], Value::Null);
        assert_eq!(
            routing_trace["pool_expansion"][0]["pool_group_id"],
            json!("pool-group-1")
        );
        assert_eq!(routing_trace["pool_expansion"][0]["key_id"], json!("key-1"));
        assert_eq!(
            routing_trace["pool_expansion"][0]["selected_order"],
            json!(1)
        );
    }
}
