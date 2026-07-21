use std::collections::BTreeMap;
use std::time::Duration;

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
use crate::ai_serving::{
    ExecutionRuntimeAuthContext, GatewayAuthApiKeySnapshot, GatewayProviderTransportSnapshot,
    PlannerAppState, CODEX_RESPONSES_LITE_HEADER,
};
use crate::cache::CacheLoadObserver;
use crate::client_session_affinity::client_session_affinity_from_api_request;
use crate::clock::current_unix_secs;
use crate::routing::{
    apply_routing_mutation_plan, build_routing_trace_seed, resolve_gateway_routing_policy,
    resolve_gateway_static_default_routing_policy, select_gateway_routing_group,
    GatewayRoutingPolicyInput, GatewayRoutingSelectionError, GatewayRoutingSelectionInput,
    GatewayStaticRoutingPolicyInput, ROUTING_GROUP_HEADER,
};
use crate::stage_metrics::observe_gateway_stage_ms;
use crate::{AiExecutionDecision, AppState, GatewayError};

// Keep normal freshness bounded for cross-node routing changes. Stale values
// are served while a single background refresh updates the cache.
const ROUTING_GROUP_SELECTION_CACHE_TTL: Duration = Duration::from_secs(30);
const ROUTING_GROUP_SELECTION_CACHE_STALE_TTL: Duration = Duration::from_secs(120);
const CODEX_ACCOUNT_ID_HEADER: &str = "chatgpt-account-id";
const CODEX_FEDRAMP_HEADER: &str = "x-openai-fedramp";

#[derive(Debug, Clone)]
pub(crate) struct ResolvedLocalDecisionAuthInput {
    pub(crate) auth_context: ExecutionRuntimeAuthContext,
    pub(crate) auth_snapshot: GatewayAuthApiKeySnapshot,
    pub(crate) required_capabilities: Option<serde_json::Value>,
    pub(crate) model_directive_policy: crate::system_features::ModelDirectivePolicySnapshot,
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
    pub(crate) model_directive_policy: crate::system_features::ModelDirectivePolicySnapshot,
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
    transport: Option<&GatewayProviderTransportSnapshot>,
) -> Result<(), GatewayError> {
    let provider_api_format = decision
        .provider_api_format
        .clone()
        .or_else(|| {
            input
                .routing_context
                .as_ref()
                .map(|context| context.client_api_format.clone())
        })
        .unwrap_or_default();
    let provider_type = decision.provider_type.clone().unwrap_or_default();
    let terminal_provider_model = decision
        .provider_request_body
        .as_ref()
        .and_then(|body| body.get("model"))
        .and_then(Value::as_str)
        .or(decision.mapped_model.as_deref())
        .or(decision.model_name.as_deref())
        .unwrap_or(input.requested_model.as_str());
    let model_capabilities = transport.and_then(|transport| {
        crate::ai_serving::codex_model_capabilities_for_transport(
            transport,
            provider_api_format.as_str(),
            terminal_provider_model,
            input.requested_model.as_str(),
        )
    });
    crate::ai_serving::apply_codex_openai_responses_lite_header_for_request_body_with_capabilities(
        &mut decision.provider_request_headers,
        decision.provider_request_body.as_ref(),
        provider_type.as_str(),
        provider_api_format.as_str(),
        terminal_provider_model,
        input.requested_model.as_str(),
        model_capabilities.as_ref(),
    );

    let Some(context) = input.routing_context.as_ref() else {
        // Cache identity headers are projected only at the terminal boundary. Any non-empty
        // session headers already present here are explicit client or header-rule inputs and stay
        // authoritative.
        if let Some(provider_request_body) = decision.provider_request_body.as_ref() {
            crate::ai_serving::apply_codex_openai_responses_identity_headers(
                &mut decision.provider_request_headers,
                provider_request_body,
                provider_type.as_str(),
                provider_api_format.as_str(),
            );
        }
        return Ok(());
    };
    let provider_body_rules = decision
        .report_context
        .as_ref()
        .and_then(|context| context.get("body_rules"))
        .cloned();
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
    let mut protected_codex_header_names = vec![CODEX_ACCOUNT_ID_HEADER, CODEX_FEDRAMP_HEADER];
    if provider_type.eq_ignore_ascii_case("codex")
        && crate::ai_serving::is_openai_responses_family_format(provider_api_format.as_str())
    {
        protected_codex_header_names.extend([
            "x-client-request-id",
            "accept",
            "content-encoding",
            CODEX_RESPONSES_LITE_HEADER,
        ]);
    }
    let protected_codex_headers = protected_codex_header_names
        .into_iter()
        .map(|name| (name, provider_headers.get(name).cloned()))
        .collect::<Vec<_>>();
    let provider_headers_json = headers_to_routing_value(&provider_headers);
    let policy = resolve_gateway_routing_policy(GatewayRoutingPolicyInput {
        group_id: context.group_id.as_deref(),
        group_version: context.group_version,
        group_config_json: &context.group_config_json,
        selection_source: context.selection_source.as_str(),
        requested_model: input.requested_model.as_str(),
        resolved_model,
        api_format: provider_api_format.as_str(),
        user_id: Some(input.auth_context.user_id.as_str()),
        api_key_id: Some(input.auth_context.api_key_id.as_str()),
        headers: &provider_headers_json,
        body: &provider_request_body,
        phase: RoutingRulePhase::ProviderRequest,
    })?;
    ensure_report_context_routing_trace(input, decision, &policy);
    if policy.mutation_plan.is_empty() {
        if let Some(provider_request_body) = decision.provider_request_body.as_ref() {
            crate::ai_serving::apply_codex_openai_responses_identity_headers(
                &mut decision.provider_request_headers,
                provider_request_body,
                provider_type.as_str(),
                provider_api_format.as_str(),
            );
        }
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
    for (name, value) in protected_codex_headers {
        provider_headers.remove(name);
        if let Some(value) = value {
            provider_headers.insert(HeaderName::from_static(name), value);
        }
    }
    if original_provider_request_body.is_some() {
        let provider_model = provider_request_body
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or(decision.mapped_model.as_deref())
            .or(decision.model_name.as_deref())
            .unwrap_or(input.requested_model.as_str())
            .to_string();
        let model_capabilities = transport.and_then(|transport| {
            crate::ai_serving::codex_model_capabilities_for_transport(
                transport,
                provider_api_format.as_str(),
                provider_model.as_str(),
                input.requested_model.as_str(),
            )
        });
        crate::ai_serving::finalize_openai_provider_request_with_codex_model_capabilities(
            &mut provider_request_body,
            crate::ai_serving::OpenAiProviderRequestFinalization {
                source_api_format: context.client_api_format.as_str(),
                provider_api_format: provider_api_format.as_str(),
                provider_type: provider_type.as_str(),
                provider_model: provider_model.as_str(),
                source_model: input.requested_model.as_str(),
                body_rules: provider_body_rules.as_ref(),
                upstream_is_stream: decision.upstream_is_stream,
                require_body_stream_field: original_provider_request_body
                    .as_ref()
                    .is_some_and(|body| body.get("stream").is_some()),
            },
            model_capabilities.as_ref(),
        )
        .map_err(|violation| GatewayError::Client {
            status: StatusCode::BAD_REQUEST,
            message: format!("routing provider_request violates provider contract: {violation:?}"),
        })?;
    }
    let provider_model = provider_request_body
        .get("model")
        .and_then(Value::as_str)
        .or(decision.mapped_model.as_deref())
        .or(decision.model_name.as_deref())
        .unwrap_or(input.requested_model.as_str());
    let mut provider_request_headers = header_map_to_btree_headers(&provider_headers);
    let model_capabilities = transport.and_then(|transport| {
        crate::ai_serving::codex_model_capabilities_for_transport(
            transport,
            provider_api_format.as_str(),
            provider_model,
            input.requested_model.as_str(),
        )
    });
    crate::ai_serving::apply_codex_openai_responses_identity_headers(
        &mut provider_request_headers,
        &provider_request_body,
        provider_type.as_str(),
        provider_api_format.as_str(),
    );
    crate::ai_serving::apply_codex_openai_responses_lite_header_for_request_body_with_capabilities(
        &mut provider_request_headers,
        Some(&provider_request_body),
        provider_type.as_str(),
        provider_api_format.as_str(),
        provider_model,
        input.requested_model.as_str(),
        model_capabilities.as_ref(),
    );
    crate::ai_serving::apply_codex_openai_compact_terminal_headers(
        &mut provider_request_headers,
        provider_type.as_str(),
        provider_api_format.as_str(),
    );
    provider_headers = btree_headers_to_header_map(&provider_request_headers)?;
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
    model_directive_policy: &'a crate::system_features::ModelDirectivePolicySnapshot,
    model_directive_base_model: Option<String>,
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
                self.model_directive_base_model.as_deref(),
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
            model_directive_policy: self.model_directive_policy.clone(),
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
        model_directive_policy: resolved_input.model_directive_policy,
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
            // Explicit non-default groups are authorized against principal
            // bindings, so both selection and its cache key must retain the
            // caller context. Only the implicit no-binding system-default
            // path is global and can skip the membership lookup.
            let principal_context_required = if explicit_group.is_some() {
                true
            } else {
                !matches!(repository.has_any_routing_group_binding().await, Ok(false))
            };
            let user_group_ids = if principal_context_required {
                let user_groups_lookup_started_at = std::time::Instant::now();
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
                observe_gateway_stage_ms(
                    "routing_user_groups_lookup",
                    user_groups_lookup_started_at.elapsed().as_millis() as u64,
                );
                user_group_ids
            } else {
                Vec::new()
            };
            let selection_user_id =
                principal_context_required.then(|| input.auth_context.user_id.clone());
            let selection_api_key_id =
                principal_context_required.then(|| input.auth_context.api_key_id.clone());
            let selection_cache_key = routing_group_selection_cache_key(
                explicit_group.as_deref(),
                selection_user_id.as_deref(),
                selection_api_key_id.as_deref(),
                &user_group_ids,
            );
            let group_selection_started_at = std::time::Instant::now();
            let selection = state
                .routing_group_selection_cache
                .get_or_load_once_stale_while_revalidating(
                    selection_cache_key,
                    ROUTING_GROUP_SELECTION_CACHE_TTL,
                    ROUTING_GROUP_SELECTION_CACHE_STALE_TTL,
                    || async {
                        let selection_load_started_at = std::time::Instant::now();
                        let selection = select_gateway_routing_group(
                            repository.as_ref(),
                            GatewayRoutingSelectionInput {
                                explicit_group: explicit_group.as_deref(),
                                user_id: selection_user_id.as_deref(),
                                api_key_id: selection_api_key_id.as_deref(),
                                user_group_ids: &user_group_ids,
                            },
                        )
                        .await
                        .map_err(routing_selection_error)?;
                        observe_gateway_stage_ms(
                            "routing_group_selection_load",
                            selection_load_started_at.elapsed().as_millis() as u64,
                        );
                        Ok::<_, GatewayError>(Some(selection))
                    },
                    || {
                        let repository = repository.clone();
                        let explicit_group = explicit_group.clone();
                        let user_id = selection_user_id.clone();
                        let api_key_id = selection_api_key_id.clone();
                        let user_group_ids = user_group_ids.clone();
                        async move {
                            let selection_load_started_at = std::time::Instant::now();
                            let selection = select_gateway_routing_group(
                                repository.as_ref(),
                                GatewayRoutingSelectionInput {
                                    explicit_group: explicit_group.as_deref(),
                                    user_id: user_id.as_deref(),
                                    api_key_id: api_key_id.as_deref(),
                                    user_group_ids: &user_group_ids,
                                },
                            )
                            .await
                            .map_err(routing_selection_error)?;
                            observe_gateway_stage_ms(
                                "routing_group_selection_load",
                                selection_load_started_at.elapsed().as_millis() as u64,
                            );
                            Ok::<_, GatewayError>(Some(selection))
                        }
                    },
                    CacheLoadObserver::default(),
                )
                .await?
                .unwrap_or_default();
            observe_gateway_stage_ms(
                "routing_group_selection",
                group_selection_started_at.elapsed().as_millis() as u64,
            );
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
        input.client_session_affinity = client_session_affinity_from_api_request(
            client_api_format,
            &parts.headers,
            Some(body_json),
        );
        input.routing_policy = None;
        input.routing_trace_seed = None;
        input.routing_context = None;
        return Ok(());
    };

    if try_attach_static_default_routing_policy_to_input(
        input,
        parts,
        body_json,
        client_api_format,
        group_id.as_deref(),
        group_version,
        &group_config_json,
        selection_source.as_str(),
    )? {
        return Ok(());
    }

    let headers_json = headers_to_routing_value(&parts.headers);
    let policy_resolve_started_at = std::time::Instant::now();
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
    observe_gateway_stage_ms(
        "routing_policy_resolve",
        policy_resolve_started_at.elapsed().as_millis() as u64,
    );
    let mut effective_body_json = body_json.clone();
    let mut effective_headers = parts.headers.clone();
    let mutation_apply_started_at = std::time::Instant::now();
    apply_routing_mutation_plan(
        &mut effective_body_json,
        &mut effective_headers,
        &policy.mutation_plan,
    )?;
    observe_gateway_stage_ms(
        "routing_mutation_apply",
        mutation_apply_started_at.elapsed().as_millis() as u64,
    );

    let mut requested_model_changed = false;
    if let Some(mut mutated_model) = extract_standard_requested_model(&effective_body_json) {
        mutated_model = mutated_model.trim().to_string();
        if !mutated_model.is_empty() && mutated_model != input.requested_model {
            input.requested_model = mutated_model;
            requested_model_changed = true;
        }
    }
    if requested_model_changed {
        let model_directive_resolution = input
            .model_directive_policy
            .resolve_reasoning(client_api_format, Some(input.requested_model.as_str()));
        input.required_capabilities = PlannerAppState::new(state)
            .resolve_request_candidate_required_capabilities(
                &input.auth_context.user_id,
                &input.auth_context.api_key_id,
                Some(input.requested_model.as_str()),
                input.required_capabilities.as_ref(),
                model_directive_resolution.base_model(),
            )
            .await;
    }

    let effective_headers_json = headers_to_routing_value(&effective_headers);
    input.client_session_affinity = client_session_affinity_from_api_request(
        client_api_format,
        &effective_headers,
        Some(&effective_body_json),
    );
    let final_policy_resolve_started_at = std::time::Instant::now();
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
    observe_gateway_stage_ms(
        "routing_policy_resolve",
        final_policy_resolve_started_at.elapsed().as_millis() as u64,
    );
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

fn try_attach_static_default_routing_policy_to_input(
    input: &mut LocalRequestedModelDecisionInput,
    parts: &http::request::Parts,
    body_json: &Value,
    client_api_format: &str,
    group_id: Option<&str>,
    group_version: Option<i64>,
    group_config_json: &Value,
    selection_source: &str,
) -> Result<bool, GatewayError> {
    let static_policy_resolve_started_at = std::time::Instant::now();
    let Some(policy) =
        resolve_gateway_static_default_routing_policy(GatewayStaticRoutingPolicyInput {
            group_id,
            group_version,
            group_config_json,
            selection_source,
            requested_model: input.requested_model.as_str(),
            resolved_model: input.requested_model.as_str(),
        })?
    else {
        observe_gateway_stage_ms(
            "routing_static_policy_resolve",
            static_policy_resolve_started_at.elapsed().as_millis() as u64,
        );
        return Ok(false);
    };
    observe_gateway_stage_ms(
        "routing_static_policy_resolve",
        static_policy_resolve_started_at.elapsed().as_millis() as u64,
    );

    input.client_session_affinity = client_session_affinity_from_api_request(
        client_api_format,
        &parts.headers,
        Some(body_json),
    );
    input.routing_trace_seed = Some(build_routing_trace_seed(&policy, client_api_format));
    input.routing_policy = Some(policy);
    input.routing_context = None;
    Ok(true)
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
    requested_model_api_format: Option<&str>,
    explicit_required_capabilities: Option<&serde_json::Value>,
    model_directive_policy: &crate::system_features::ModelDirectivePolicySnapshot,
) -> Result<Option<ResolvedLocalDecisionAuthInput>, GatewayError> {
    let model_directive_base_model = match (requested_model, requested_model_api_format) {
        (Some(model), Some(api_format)) => model_directive_policy
            .resolve_reasoning(api_format, Some(model))
            .base_model()
            .map(str::to_owned),
        _ => None,
    };
    let port = GatewayAuthenticatedDecisionInputPort {
        state: PlannerAppState::new(state),
        now_unix_secs: current_unix_secs(),
        model_directive_policy,
        model_directive_base_model,
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

fn routing_group_selection_cache_key(
    explicit_group: Option<&str>,
    user_id: Option<&str>,
    api_key_id: Option<&str>,
    user_group_ids: &[String],
) -> String {
    let groups = user_group_ids
        .iter()
        .map(|value| escape_cache_key_part(value))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "v1|explicit={}|user={}|api_key={}|groups={}",
        escape_cache_key_part(explicit_group.unwrap_or_default()),
        escape_cache_key_part(user_id.unwrap_or_default()),
        escape_cache_key_part(api_key_id.unwrap_or_default()),
        groups
    )
}

fn escape_cache_key_part(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace('|', "%7C")
        .replace(',', "%2C")
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
    use std::sync::Arc;

    use super::*;
    use aether_data::repository::routing_profiles::InMemoryRoutingGroupRepository;
    use aether_data_contracts::repository::routing_profiles::{
        CreateRoutingGroupBindingRecord, CreateRoutingGroupRecord, RoutingGroupBindingSubject,
        RoutingGroupWriteRepository,
    };
    use aether_provider_transport::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider,
    };

    #[test]
    fn explicit_routing_selection_cache_key_is_principal_specific() {
        let first = routing_group_selection_cache_key(
            Some("private"),
            Some("user-1"),
            Some("key-1"),
            &["team-1".to_string()],
        );
        let second = routing_group_selection_cache_key(
            Some("private"),
            Some("user-2"),
            Some("key-2"),
            &["team-2".to_string()],
        );

        assert_ne!(first, second);
        assert!(first.contains("user=user-1"));
        assert!(first.contains("api_key=key-1"));
        assert!(first.contains("groups=team-1"));
    }

    #[tokio::test]
    async fn explicit_routing_attachment_authorizes_and_caches_per_principal() {
        let repository = Arc::new(InMemoryRoutingGroupRepository::default());
        repository
            .create_routing_group(CreateRoutingGroupRecord {
                id: "private-group".to_string(),
                name: "private".to_string(),
                description: None,
                enabled: true,
                is_system_default: false,
                config_json: json!({}),
                version: 1,
                created_at: 1,
                updated_at: 1,
                published_at: None,
            })
            .await
            .unwrap();
        repository
            .create_routing_group_binding(CreateRoutingGroupBindingRecord {
                id: "binding-user-1".to_string(),
                group_id: "private-group".to_string(),
                subject_type: RoutingGroupBindingSubject::User,
                subject_id: "user-1".to_string(),
                is_default: false,
                allow_explicit_select: true,
                created_at: 1,
                updated_at: 1,
            })
            .await
            .unwrap();
        let state = AppState::new().unwrap().with_data_state_for_tests(
            crate::data::GatewayDataState::disabled()
                .with_routing_group_repository_for_tests(repository),
        );
        let (parts, _) = http::Request::builder()
            .header(ROUTING_GROUP_HEADER, "private-group")
            .body(())
            .unwrap()
            .into_parts();

        let mut allowed = sample_decision_input();
        attach_routing_policy_to_local_requested_model_input(
            &state,
            &parts,
            &mut allowed,
            &json!({"model": "gpt-5"}),
            "openai:chat",
        )
        .await
        .expect("bound principal should explicitly select the private group");
        let policy = allowed
            .routing_policy
            .as_ref()
            .expect("explicit selection should attach routing policy");
        assert_eq!(policy.group_id.as_deref(), Some("private-group"));
        assert_eq!(policy.selection_source, "explicit_header");

        let mut denied = sample_decision_input();
        denied.auth_context.user_id = "user-2".to_string();
        denied.auth_context.api_key_id = "api-key-2".to_string();
        let error = attach_routing_policy_to_local_requested_model_input(
            &state,
            &parts,
            &mut denied,
            &json!({"model": "gpt-5"}),
            "openai:chat",
        )
        .await
        .expect_err("another principal must not reuse the authorized cache entry");
        match error {
            GatewayError::Client { status, message } => {
                assert_eq!(status, StatusCode::FORBIDDEN);
                assert!(message.contains("not allowed for this principal"));
            }
            other => panic!("unexpected explicit selection error: {other:?}"),
        }
    }

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
            model_directive_policy: Default::default(),
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
            provider_type: Some("openai".to_string()),
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

    fn sample_codex_transport_with_card() -> GatewayProviderTransportSnapshot {
        let card = json!({
            "id": "gpt-future-agent",
            "slug": "gpt-future-agent",
            "use_responses_lite": true,
            "supports_reasoning_summary_parameter": true,
            "default_reasoning_level": "low",
            "default_reasoning_summary": "none",
            "supported_reasoning_levels": [{"effort": "low"}, {"effort": "high"}]
        });
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-codex".to_string(),
                name: "Codex".to_string(),
                provider_type: "codex".to_string(),
                website: None,
                is_active: true,
                keep_priority_on_conversion: false,
                enable_format_conversion: true,
                concurrent_limit: None,
                max_retries: None,
                proxy: None,
                request_timeout_secs: None,
                stream_first_byte_timeout_secs: None,
                config: None,
            },
            endpoint: GatewayProviderTransportEndpoint {
                id: "endpoint-codex".to_string(),
                provider_id: "provider-codex".to_string(),
                api_format: "openai:responses:compact".to_string(),
                api_family: Some("openai".to_string()),
                endpoint_kind: Some("compact".to_string()),
                is_active: true,
                base_url: "https://chatgpt.com/backend-api/codex".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: None,
                config: None,
                format_acceptance_config: None,
                proxy: None,
            },
            key: GatewayProviderTransportKey {
                id: "key-codex".to_string(),
                provider_id: "provider-codex".to_string(),
                name: "Codex key".to_string(),
                auth_type: "oauth".to_string(),
                is_active: true,
                api_formats: Some(vec!["openai:responses:compact".to_string()]),
                auth_type_by_format: None,
                allow_auth_channel_mismatch_formats: None,
                allowed_models: Some(vec!["gpt-future-agent".to_string()]),
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: None,
                expires_at_unix_secs: None,
                proxy: None,
                fingerprint: None,
                upstream_metadata: Some(crate::ai_serving::build_codex_model_catalog_metadata(&[
                    card,
                ])),
                decrypted_api_key: "access-token".to_string(),
                decrypted_auth_config: None,
            },
        }
    }

    fn set_provider_request_rules(
        input: &mut LocalRequestedModelDecisionInput,
        allowed_models: &[&str],
        actions: Value,
    ) {
        let config = json!({
            "allowed_models": allowed_models,
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
    fn static_default_routing_policy_attaches_without_request_context() {
        let request = http::Request::builder()
            .header("content-type", "application/json")
            .body(())
            .expect("request should build");
        let (parts, _) = request.into_parts();
        let mut input = LocalRequestedModelDecisionInput {
            auth_context: sample_auth_context(),
            requested_model: "mock-model".to_string(),
            auth_snapshot: sample_auth_snapshot(),
            required_capabilities: None,
            request_auth_channel: None,
            client_session_affinity: None,
            routing_policy: None,
            routing_trace_seed: None,
            model_directive_policy: Default::default(),
            routing_context: Some(LocalRoutingRequestContext {
                group_id: Some("stale".to_string()),
                group_version: Some(1),
                group_config_json: json!({}),
                selection_source: "stale".to_string(),
                client_api_format: "openai:chat".to_string(),
                effective_body_json: json!({}),
                effective_headers: HeaderMap::new(),
            }),
        };
        let group_config_json = json!({
            "default_policy": {
                "priority_mode": "global_key",
                "scheduling_mode": "load_balance",
                "keep_priority_on_conversion": true
            },
            "allowed_models": [],
            "model_policies": [],
            "rules": []
        });

        let attached = try_attach_static_default_routing_policy_to_input(
            &mut input,
            &parts,
            &json!({"model": "mock-model"}),
            "openai:chat",
            Some("group-1"),
            Some(4),
            &group_config_json,
            "system_default",
        )
        .expect("static routing should attach");

        assert!(attached);
        assert!(input.routing_context.is_none());
        let policy = input.routing_policy.as_ref().expect("policy should be set");
        assert_eq!(policy.group_id.as_deref(), Some("group-1"));
        assert_eq!(policy.group_version, Some(4));
        assert_eq!(
            policy.priority_mode,
            aether_routing_core::RoutingSetPriorityMode::GlobalKey
        );
        assert_eq!(
            policy.scheduling_mode,
            aether_routing_core::RoutingSchedulingMode::LoadBalance
        );
        assert!(policy.keep_priority_on_conversion);
        assert!(policy.mutation_plan.is_empty());
        assert!(input.routing_trace_seed.is_some());
    }

    #[test]
    fn dynamic_routing_policy_does_not_attach_static_fast_path() {
        let request = http::Request::builder()
            .body(())
            .expect("request should build");
        let (parts, _) = request.into_parts();
        let mut input = LocalRequestedModelDecisionInput {
            auth_context: sample_auth_context(),
            requested_model: "mock-model".to_string(),
            auth_snapshot: sample_auth_snapshot(),
            required_capabilities: None,
            request_auth_channel: None,
            client_session_affinity: None,
            routing_policy: None,
            routing_trace_seed: None,
            routing_context: None,
            model_directive_policy: Default::default(),
        };
        let group_config_json = json!({
            "rules": [{
                "id": "rule-1",
                "conditions": {},
                "actions": [{
                    "type": "restrict_providers",
                    "provider_ids": ["provider-1"]
                }]
            }]
        });

        let attached = try_attach_static_default_routing_policy_to_input(
            &mut input,
            &parts,
            &json!({"model": "mock-model"}),
            "openai:chat",
            Some("group-1"),
            Some(4),
            &group_config_json,
            "system_default",
        )
        .expect("dynamic config should not fail static detection");

        assert!(!attached);
        assert!(input.routing_policy.is_none());
        assert!(input.routing_trace_seed.is_none());
        assert!(input.routing_context.is_none());
    }

    #[test]
    fn provider_request_routing_policy_mutates_decision_body_headers_and_report_context() {
        let input = sample_decision_input();
        let mut decision = sample_decision();

        apply_provider_request_routing_policy_to_decision(&input, &mut decision, None)
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
    fn codex_prompt_cache_identity_headers_are_terminal_after_routing_mutations() {
        let mut input = sample_decision_input();
        input
            .routing_context
            .as_mut()
            .expect("routing context")
            .client_api_format = "openai:responses".to_string();
        set_provider_request_rules(
            &mut input,
            &["gpt-5"],
            json!([{
                "type": "patch_headers",
                "patch": [
                    {"op": "remove", "name": "session-id"},
                    {"op": "remove", "name": "thread-id"}
                ]
            }]),
        );
        let identity = "172c39e6-c0a0-5a70-8b63-e0f8e0d185a3";
        let mut decision = sample_decision();
        decision.provider_type = Some("codex".to_string());
        decision.provider_api_format = Some("openai:responses".to_string());
        decision.client_api_format = Some("openai:responses".to_string());
        decision.provider_request_body = Some(json!({
            "model": "gpt-5",
            "input": [],
            "prompt_cache_key": identity,
            "client_metadata": {
                "session_id": identity,
                "thread_id": identity
            }
        }));
        assert!(!decision.provider_request_headers.contains_key("session-id"));
        assert!(!decision.provider_request_headers.contains_key("thread-id"));

        apply_provider_request_routing_policy_to_decision(&input, &mut decision, None)
            .expect("terminal Codex identity contract should be restored");

        assert_eq!(
            decision
                .provider_request_headers
                .get("session-id")
                .map(String::as_str),
            Some(identity)
        );
        assert_eq!(
            decision
                .provider_request_headers
                .get("thread-id")
                .map(String::as_str),
            Some(identity)
        );
    }

    #[test]
    fn codex_prompt_cache_identity_headers_fail_closed_after_body_identity_removal() {
        let mut input = sample_decision_input();
        input
            .routing_context
            .as_mut()
            .expect("routing context")
            .client_api_format = "openai:responses".to_string();
        set_provider_request_rules(
            &mut input,
            &["gpt-5"],
            json!([{
                "type": "json_patch_body",
                "patch": [
                    {"op": "remove", "path": "/prompt_cache_key"},
                    {"op": "remove", "path": "/client_metadata"}
                ]
            }]),
        );
        let identity = "172c39e6-c0a0-5a70-8b63-e0f8e0d185a3";
        let mut decision = sample_decision();
        decision.provider_type = Some("codex".to_string());
        decision.provider_api_format = Some("openai:responses".to_string());
        decision.client_api_format = Some("openai:responses".to_string());
        decision.provider_request_body = Some(json!({
            "model": "gpt-5",
            "input": [],
            "prompt_cache_key": identity,
            "client_metadata": {
                "session_id": identity,
                "thread_id": identity
            }
        }));
        assert!(!decision.provider_request_headers.contains_key("session-id"));
        assert!(!decision.provider_request_headers.contains_key("thread-id"));

        apply_provider_request_routing_policy_to_decision(&input, &mut decision, None)
            .expect("terminal Codex identity contract should fail closed");

        let body = decision.provider_request_body.as_ref().expect("body");
        assert!(body.get("prompt_cache_key").is_none());
        assert!(body.get("client_metadata").is_none());
        assert!(!decision.provider_request_headers.contains_key("session-id"));
        assert!(!decision.provider_request_headers.contains_key("thread-id"));
    }

    #[test]
    fn codex_compact_contract_is_terminal_after_routing_mutations() {
        let mut input = sample_decision_input();
        input
            .routing_context
            .as_mut()
            .expect("routing context")
            .client_api_format = "openai:responses:compact".to_string();
        set_provider_request_rules(
            &mut input,
            &["gpt-5"],
            json!([
                {
                    "type": "json_patch_body",
                    "patch": [
                        {"op": "add", "path": "/store", "value": true},
                        {"op": "add", "path": "/top_logprobs", "value": 5},
                        {"op": "add", "path": "/custom_extension", "value": true},
                        {"op": "replace", "path": "/input", "value": "routed compact input"},
                        {"op": "replace", "path": "/tools", "value": [{
                            "type": "function",
                            "name": "lookup",
                            "cache_control": {"type": "ephemeral"}
                        }]}
                    ]
                },
                {
                    "type": "patch_headers",
                    "patch": [
                        {"op": "set", "name": "chatgpt-account-id", "value": "spoofed"},
                        {"op": "set", "name": "x-openai-fedramp", "value": "false"},
                        {"op": "set", "name": "x-client-request-id", "value": "spoofed"},
                        {"op": "set", "name": "accept", "value": "text/event-stream"},
                        {"op": "set", "name": "content-encoding", "value": "zstd"}
                    ]
                }
            ]),
        );
        let mut decision = sample_decision();
        decision.provider_type = Some("codex".to_string());
        decision.provider_api_format = Some("openai:responses:compact".to_string());
        decision.client_api_format = Some("openai:responses:compact".to_string());
        decision.provider_request_body = Some(json!({
            "model": "gpt-5",
            "input": [],
            "tools": [{"type": "function", "name": "lookup"}]
        }));
        decision
            .provider_request_headers
            .insert(CODEX_ACCOUNT_ID_HEADER.to_string(), "account-1".to_string());
        decision
            .provider_request_headers
            .insert(CODEX_FEDRAMP_HEADER.to_string(), "true".to_string());

        apply_provider_request_routing_policy_to_decision(&input, &mut decision, None)
            .expect("terminal contract should accept the projected request");

        let body = decision.provider_request_body.as_ref().expect("body");
        assert_eq!(body["parallel_tool_calls"], false);
        assert_eq!(body["input"][0]["type"], "message");
        assert_eq!(
            body["input"][0]["content"][0]["text"],
            "routed compact input"
        );
        assert!(body["tools"][0].get("cache_control").is_none());
        for field in ["store", "top_logprobs", "custom_extension"] {
            assert!(
                body.get(field).is_none(),
                "unexpected Compact field: {field}"
            );
        }
        assert_eq!(
            decision
                .provider_request_headers
                .get(CODEX_ACCOUNT_ID_HEADER),
            Some(&"account-1".to_string())
        );
        assert_eq!(
            decision.provider_request_headers.get(CODEX_FEDRAMP_HEADER),
            Some(&"true".to_string())
        );
        for header in ["x-client-request-id", "accept", "content-encoding"] {
            assert!(
                !decision.provider_request_headers.contains_key(header),
                "unexpected Compact header: {header}"
            );
        }
    }

    #[test]
    fn codex_responses_lite_contract_is_terminal_after_routing_mutations() {
        let mut input = sample_decision_input();
        input.requested_model = "gpt-future-agent".to_string();
        input
            .routing_context
            .as_mut()
            .expect("routing context")
            .client_api_format = "openai:responses:compact".to_string();
        set_provider_request_rules(
            &mut input,
            &["gpt-future-agent"],
            json!([
                {
                    "type": "json_patch_body",
                    "patch": [
                        {"op": "replace", "path": "/input", "value": "routed compact input"},
                        {"op": "add", "path": "/instructions", "value": "Routed instructions"},
                        {"op": "replace", "path": "/tools", "value": [{
                            "type": "function",
                            "name": "lookup",
                            "parameters": {},
                            "cache_control": {"type": "ephemeral"}
                        }]},
                        {"op": "add", "path": "/parallel_tool_calls", "value": true},
                        {"op": "add", "path": "/reasoning", "value": {
                            "effort": "high",
                            "context": "current_turn"
                        }}
                    ]
                },
                {
                    "type": "patch_headers",
                    "patch": [{
                        "op": "set",
                        "name": "x-openai-internal-codex-responses-lite",
                        "value": "false"
                    }]
                }
            ]),
        );
        let mut decision = sample_decision();
        decision.provider_type = Some("codex".to_string());
        decision.provider_api_format = Some("openai:responses:compact".to_string());
        decision.client_api_format = Some("openai:responses:compact".to_string());
        decision.mapped_model = Some("gpt-future-agent".to_string());
        decision.provider_request_body = Some(json!({
            "model": "gpt-future-agent",
            "input": [],
            "tools": []
        }));
        let transport = sample_codex_transport_with_card();

        apply_provider_request_routing_policy_to_decision(&input, &mut decision, Some(&transport))
            .expect("terminal Lite contract should accept the projected request");

        let body = decision.provider_request_body.as_ref().expect("body");
        assert_eq!(body["input"][0]["type"], "additional_tools");
        assert_eq!(body["input"][0]["tools"][0]["name"], "lookup");
        assert!(body["input"][0]["tools"][0].get("cache_control").is_none());
        assert_eq!(body["input"][1]["role"], "developer");
        assert_eq!(
            body["input"][1]["content"][0]["text"],
            "Routed instructions"
        );
        assert_eq!(body["input"][2]["role"], "user");
        assert_eq!(
            body["input"][2]["content"][0]["text"],
            "routed compact input"
        );
        assert!(body.get("tools").is_none());
        assert!(body.get("instructions").is_none());
        assert_eq!(body["parallel_tool_calls"], false);
        assert_eq!(body["reasoning"]["effort"], "high");
        assert_eq!(body["reasoning"]["context"], "all_turns");
        assert_eq!(
            decision
                .provider_request_headers
                .get(CODEX_RESPONSES_LITE_HEADER)
                .map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn provider_request_routing_policy_rejects_body_patch_without_json_body() {
        let input = sample_decision_input();
        let mut decision = sample_decision();
        decision.provider_request_body = None;
        decision.provider_request_body_base64 = Some("AA==".to_string());

        let error = apply_provider_request_routing_policy_to_decision(&input, &mut decision, None)
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
            &["gpt-5"],
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

        apply_provider_request_routing_policy_to_decision(&input, &mut decision, None)
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

        apply_provider_request_routing_policy_to_decision(&input, &mut decision, None)
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
