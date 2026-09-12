//! Client-side Responses WebSocket event forwarding and follow-up planning.

use aether_provider_transport::CodexFingerprintConvergenceContext;
use axum::extract::ws::{Message as AxumWsMessage, WebSocket};
use futures_util::SinkExt;
use serde_json::Value;
use uuid::Uuid;
use wreq::ws::message::Message as WreqWsMessage;

use super::adapter::{resolve_responses_websocket_adapter, ResponsesWebSocketDrainDirective};
use super::control::{resolve_responses_websocket_turn_control, ResponsesWebSocketTurnControl};
use super::lifecycle::{
    await_pending_turn_finalization, queue_turn_finalization,
    send_responses_websocket_turn_start_error,
};
use super::ownership::{
    await_owned_responses_websocket_plan, begin_responses_websocket_turn_with_planned_lease,
    spawn_owned_responses_websocket_plan, OwnedResponsesWebSocketDecision,
};
use super::plan_admission::{
    acquire_responses_websocket_plan_admission, send_responses_websocket_plan_admission_error,
};
use super::quota::mark_active_response_retry_unsafe;
use super::redaction::redact_responses_websocket_client_event_with_reasoning_replay_policy;
use super::request::{
    build_planning_parts, changed_followup_response_create_model,
    planned_request_uses_codex_responses_lite, planned_response_create_event,
    prepare_responses_lite_continuation, provider_model_from_decision,
    response_create_has_previous_response_id, response_create_model_or_current,
    validate_response_create_previous_response_id, validate_response_create_stream_id_support,
    validated_named_stream_id, ResponsesLiteStaticConfig,
};
use super::state::BoundResponsesConnection;
use super::turn::{
    prepare_responses_websocket_turn_decision, ResponsesWebSocketTurnObservation,
    ResponsesWebSocketTurnOutcome,
};
use super::turn_state::LogicalTurn;
use super::upstream::{
    bind_responses_upstream, decision_bound_upstream_change_fields, decision_reuses_bound_upstream,
    send_responses_websocket_upstream_message, ResponsesWebSocketUpstreamBindError,
    ResponsesWebSocketUpstreamSendError,
};
use crate::ai_serving::ResponsesWebSocketPinnedCandidate;
use crate::clock::current_unix_secs;
use crate::control::GatewayControlDecision;
use crate::handlers::proxy::websocket::ingress::WebSocketRequestContext;
use crate::handlers::proxy::websocket::session::{CLOSE_INTERNAL_ERROR, WEBSOCKET_LOG_TRANSPORT};
use crate::handlers::proxy::websocket::transport::{
    close_client_socket, close_upstream_socket, send_client_message, send_gateway_error,
    send_gateway_error_with_status, send_gateway_error_with_stream_id,
    send_responses_websocket_error_with_param,
};
use crate::plan_usage_policy::PlanUsagePolicySnapshot;
use crate::privacy::RedactionSession;
use crate::rate_limit::FrontdoorUserRpmOutcome;
use crate::AppState;

const LOG_TARGET: &str = "aether_gateway::handlers::proxy::responses_ws";
const CLOSE_UNSUPPORTED_DATA: u16 = 1003;

macro_rules! debug {
    ($($arg:tt)*) => {
        tracing::debug!(target: LOG_TARGET, $($arg)*)
    };
}

macro_rules! warn {
    ($($arg:tt)*) => {
        tracing::warn!(target: LOG_TARGET, $($arg)*)
    };
}

pub(super) enum RelayDisposition {
    Continue,
    Close,
    UpstreamError(&'static str),
    PlanUsagePermitLost,
}

pub(super) fn adapter_drain_ready(
    pending_adapter_drain: Option<ResponsesWebSocketDrainDirective>,
    response_in_flight: bool,
    observation: Option<ResponsesWebSocketTurnObservation>,
    upstream_closed: bool,
) -> bool {
    pending_adapter_drain.is_some()
        && (upstream_closed
            || !response_in_flight
            || matches!(
                observation,
                Some(ResponsesWebSocketTurnObservation::Terminal(_))
            ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResponseCreateParseError {
    code: &'static str,
    stream_id: Option<String>,
}

impl ResponseCreateParseError {
    fn new(code: &'static str, event: Option<&Value>) -> Self {
        // A syntactically valid named lane is safe to reflect on every
        // request-scoped error, even when another response.create field fails
        // validation first. Invalid lane values never pass this helper and
        // therefore cannot be reflected.
        let stream_id = event
            .and_then(validated_named_stream_id)
            .map(str::to_string);
        Self { code, stream_id }
    }
}

fn parse_response_create_event(text: &str) -> Result<Value, ResponseCreateParseError> {
    let event = serde_json::from_str::<Value>(text)
        .map_err(|_| ResponseCreateParseError::new("invalid_response_create", None))?;
    if event.as_object().is_none() {
        return Err(ResponseCreateParseError::new(
            "invalid_response_create",
            Some(&event),
        ));
    }
    if event.get("type").and_then(Value::as_str) != Some("response.create") {
        return Err(ResponseCreateParseError::new(
            "expected_response_create",
            Some(&event),
        ));
    }
    validate_response_create_previous_response_id(&event)
        .map_err(|code| ResponseCreateParseError::new(code, Some(&event)))?;
    validate_response_create_stream_id_support(&event)
        .map_err(|code| ResponseCreateParseError::new(code, Some(&event)))?;
    Ok(event)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContinuationConstraint {
    Pinned,
    UnknownResponseId,
    UpstreamUnavailable,
    ModelChangeUnsupported,
}

/// Classifies only response-chain turns. Independent turns return `None` and
/// remain free to use the normal planner. A non-null `previous_response_id`
/// must never fall through to that path: its state belongs to the connection
/// and provider key that produced it.
fn continuation_constraint(
    event: &Value,
    current_client_model: &str,
    upstream_available: bool,
    response_id_owned_by_connection: bool,
) -> Result<Option<ContinuationConstraint>, &'static str> {
    if !response_create_has_previous_response_id(event) {
        return Ok(None);
    }
    if !response_id_owned_by_connection {
        return Ok(Some(ContinuationConstraint::UnknownResponseId));
    }
    if changed_followup_response_create_model(event, current_client_model)?.is_some() {
        return Ok(Some(ContinuationConstraint::ModelChangeUnsupported));
    }
    if !upstream_available {
        return Ok(Some(ContinuationConstraint::UpstreamUnavailable));
    }
    Ok(Some(ContinuationConstraint::Pinned))
}

fn pinned_continuation_planning_event(
    client_event: &Value,
    current_client_model: &str,
) -> Result<Value, &'static str> {
    let mut planning_event = client_event.clone();
    // Validate an explicitly supplied value before replacing it with the
    // canonical bound model. The routing decision has already established
    // that this is not a model-changing continuation, but the planner must not
    // receive a whitespace or case variant that fails exact mapping lookup.
    response_create_model_or_current(&mut planning_event, current_client_model)?;
    planning_event
        .as_object_mut()
        .ok_or("invalid_response_create")?
        .insert(
            "model".to_string(),
            Value::String(current_client_model.to_string()),
        );
    Ok(planning_event)
}

pub(super) async fn forward_client_message(
    client_message: AxumWsMessage,
    bound: &mut BoundResponsesConnection,
    client_socket: &mut WebSocket,
    state: &AppState,
    context: &WebSocketRequestContext,
) -> RelayDisposition {
    match client_message {
        AxumWsMessage::Text(text) => {
            let text = text.to_string();
            let mut client_event = match parse_response_create_event(&text) {
                Ok(event) => event,
                Err(error) => {
                    let message = match error.code {
                        "invalid_response_create_previous_response_id" => {
                            "response.create.previous_response_id must be null or a non-empty string"
                        }
                        "invalid_response_create_stream_id" => {
                            "response.create.stream_id must be 1-256 ASCII letters, numbers, underscores, hyphens, or periods"
                        }
                        "responses_websocket_named_stream_unsupported" => {
                            "Aether currently supports only the implicit default WebSocket lane; omit response.create.stream_id"
                        }
                        _ => "WebSocket client text events must be response.create JSON objects",
                    };
                    send_gateway_error_with_stream_id(
                        client_socket,
                        error.code,
                        message,
                        error.stream_id.as_deref(),
                    )
                    .await;
                    return RelayDisposition::Continue;
                }
            };

            if !bound.turn_state.accepts_new_response_create() {
                send_gateway_error(
                    client_socket,
                    "response_already_in_progress",
                    "This connection runs one response at a time",
                )
                .await;
                return RelayDisposition::Continue;
            }

            // A prior terminal turn may still be writing usage/audit and
            // projecting provider effects. Do not let a new independent turn
            // plan against stale health, adaptive, or pool state.
            await_pending_turn_finalization(bound).await;

            let requested_model =
                match response_create_model_or_current(&mut client_event, &bound.client_model) {
                    Ok(model) => model,
                    Err(code) => {
                        send_gateway_error(
                            client_socket,
                            code,
                            "response.create.model must be a non-empty string",
                        )
                        .await;
                        return RelayDisposition::Continue;
                    }
                };

            // Build the per-turn planning shape before any policy check, then
            // derive one strong live control snapshot that every stage below
            // shares. The connection's Upgrade-time decision is only the
            // immutable identity seed.
            let logical_turn_id = Uuid::now_v7().to_string();
            let mut planning_parts = build_planning_parts(context);
            let codex_fingerprint_context =
                crate::ai_serving::codex_context::attach_codex_logical_turn_context(
                    &mut planning_parts,
                    &client_event,
                    &logical_turn_id,
                );
            let turn_control = match resolve_responses_websocket_turn_control(
                state,
                context,
                &planning_parts,
                &client_event,
            )
            .await
            {
                Ok(control) => control,
                Err(error) => {
                    warn!(
                        event_name = "responses_websocket_followup_turn_control_rejected",
                        log_type = "ops",
                        transport = WEBSOCKET_LOG_TRANSPORT,
                        websocket = true,
                        trace_id = %context.trace_id,
                        error = ?error,
                        "gateway rejected a Responses WebSocket follow-up after live policy refresh"
                    );
                    send_responses_websocket_turn_start_error(client_socket, &error).await;
                    return RelayDisposition::Continue;
                }
            };

            match consume_response_create_rate_limit(
                state,
                &turn_control.decision,
                turn_control.rpm_bypassed,
            )
            .await
            {
                Ok(true) => {}
                Ok(false) => {
                    send_gateway_error_with_status(
                        client_socket,
                        429,
                        "rate_limit_exceeded",
                        "Too many response.create events; retry later",
                    )
                    .await;
                    return RelayDisposition::Continue;
                }
                Err(()) => {
                    send_gateway_error_with_status(
                        client_socket,
                        503,
                        "gateway_rate_limit_unavailable",
                        "Gateway could not evaluate the response rate limit",
                    )
                    .await;
                    close_client_socket(
                        client_socket,
                        CLOSE_INTERNAL_ERROR,
                        "rate_limit_unavailable",
                    )
                    .await;
                    return RelayDisposition::Close;
                }
            }

            // Subscription-plan concurrency is admitted per logical
            // response.create. Keep the permit and policy snapshot attached
            // to this turn through any transparent provider rebind.
            let plan_usage_admission = match acquire_responses_websocket_plan_admission(
                state,
                &turn_control.decision,
                &logical_turn_id,
            )
            .await
            {
                Ok(admission) => admission,
                Err(error) => {
                    warn!(
                        event_name = "responses_websocket_followup_plan_usage_rejected",
                        log_type = "event",
                        transport = WEBSOCKET_LOG_TRANSPORT,
                        websocket = true,
                        trace_id = %context.trace_id,
                        logical_turn_id = %logical_turn_id,
                        error = ?error,
                        "gateway rejected a Responses WebSocket follow-up at its subscription plan limit"
                    );
                    send_responses_websocket_plan_admission_error(client_socket, &error).await;
                    return RelayDisposition::Continue;
                }
            };
            let crate::plan_usage_policy::PlanUsageAdmission {
                permit: plan_usage_permit,
                policy_snapshot: plan_usage_policy_snapshot,
            } = plan_usage_admission;

            let response_id_owned_by_connection = client_event
                .get("previous_response_id")
                .and_then(Value::as_str)
                .is_none_or(|response_id| bound.continuation_response_ids.contains(response_id));
            let is_pinned_continuation = match continuation_constraint(
                &client_event,
                &bound.client_model,
                bound.upstream.is_some(),
                response_id_owned_by_connection,
            ) {
                Ok(Some(ContinuationConstraint::Pinned)) => true,
                Ok(Some(ContinuationConstraint::UnknownResponseId)) => {
                    send_responses_websocket_error_with_param(
                        client_socket,
                        400,
                        "invalid_request_error",
                        "previous_response_not_found",
                        "The previous response is unavailable on this authenticated WebSocket connection",
                        "previous_response_id",
                    )
                    .await;
                    return RelayDisposition::Continue;
                }
                Ok(Some(ContinuationConstraint::UpstreamUnavailable)) => {
                    send_gateway_error_with_status(
                        client_socket,
                        503,
                        "responses_continuation_provider_unavailable",
                        "The bound provider connection is unavailable for this continuation",
                    )
                    .await;
                    return RelayDisposition::Continue;
                }
                Ok(Some(ContinuationConstraint::ModelChangeUnsupported)) => {
                    send_gateway_error_with_status(
                        client_socket,
                        409,
                        "responses_continuation_model_change_unsupported",
                        "A continuation cannot change models on the bound provider connection",
                    )
                    .await;
                    return RelayDisposition::Continue;
                }
                Ok(None) => false,
                Err(code) => {
                    send_gateway_error(
                        client_socket,
                        code,
                        "response.create.model must be a non-empty string",
                    )
                    .await;
                    return RelayDisposition::Continue;
                }
            };

            // Static Responses Lite configuration belongs to the response
            // chain, not to a redaction TTL bucket. Compare and strip it from
            // continuation input while it is still the raw client value; only
            // the incremental event is redacted below. Independent turns keep
            // a raw hash so a later sentinel rotation cannot look like a tools
            // or instructions change.
            let raw_responses_lite_static_config = (!is_pinned_continuation)
                .then(|| ResponsesLiteStaticConfig::from_response_create(&client_event));
            let client_event = if is_pinned_continuation {
                if let Some(static_config) = bound.responses_lite_static_config.as_ref() {
                    match prepare_responses_lite_continuation(&client_event, static_config) {
                        Ok(event) => event,
                        Err("responses_lite_continuation_static_config_changed") => {
                            send_gateway_error_with_status(
                                client_socket,
                                409,
                                "responses_lite_continuation_static_config_changed",
                                "Responses Lite tools or instructions changed; start a new response without previous_response_id",
                            )
                            .await;
                            return RelayDisposition::Continue;
                        }
                        Err(code) => {
                            send_gateway_error(
                                client_socket,
                                code,
                                "Gateway could not validate the Responses Lite continuation",
                            )
                            .await;
                            return RelayDisposition::Continue;
                        }
                    }
                } else {
                    client_event
                }
            } else {
                client_event
            };

            // 这一轮的 planning Parts 只构造一次（它携带 per-turn 的
            // RedactionSessionSlot），并且客户端事件也只在这里脱敏一次。
            // continuation 已经去掉继承的 static prefix，所以只 mask 新增 input；
            // 后续 re-plan / continuation / 配额重试都只看脱敏后的增量事件。
            let redacted_client_event =
                redact_responses_websocket_client_event_with_reasoning_replay_policy(
                    state,
                    &planning_parts,
                    &turn_control.decision,
                    &client_event,
                    bound.body_normalization.reasoning_replay_policy(),
                )
                .await;
            let (client_event, turn_redaction_session) = match redacted_client_event {
                Ok(Some(redaction)) => (redaction.client_event, Some(redaction.session)),
                Ok(None) => (client_event, None),
                Err(error) => {
                    warn!(
                        event_name = "responses_websocket_followup_redaction_failed",
                        log_type = "ops",
                        transport = WEBSOCKET_LOG_TRANSPORT,
                        websocket = true,
                        trace_id = %context.trace_id,
                        error = ?error,
                        "gateway could not apply chat PII redaction to a Responses WebSocket turn"
                    );
                    send_gateway_error_with_status(
                        client_socket,
                        500,
                        "responses_websocket_redaction_unavailable",
                        "Gateway could not apply the configured PII redaction",
                    )
                    .await;
                    close_client_socket(
                        client_socket,
                        CLOSE_INTERNAL_ERROR,
                        "responses_websocket_redaction_unavailable",
                    )
                    .await;
                    return RelayDisposition::Close;
                }
            };
            if is_pinned_continuation {
                return forward_pinned_continuation(
                    bound,
                    client_socket,
                    state,
                    context,
                    planning_parts,
                    client_event,
                    logical_turn_id,
                    codex_fingerprint_context,
                    turn_control,
                    turn_redaction_session,
                    plan_usage_permit,
                    plan_usage_policy_snapshot,
                )
                .await;
            }
            forward_replanned_response_create(
                bound,
                client_socket,
                state,
                context,
                planning_parts,
                client_event,
                requested_model,
                logical_turn_id,
                codex_fingerprint_context,
                turn_control,
                raw_responses_lite_static_config
                    .expect("independent turns always retain their raw static config"),
                turn_redaction_session,
                plan_usage_permit,
                plan_usage_policy_snapshot,
            )
            .await
        }
        AxumWsMessage::Binary(_) => {
            mark_active_response_retry_unsafe(bound, "client_binary_frame");
            send_gateway_error(
                client_socket,
                "responses_websocket_binary_frame_unsupported",
                "Responses WebSocket mode accepts text events only",
            )
            .await;
            close_client_socket(client_socket, CLOSE_UNSUPPORTED_DATA, "unsupported_data").await;
            RelayDisposition::Close
        }
        AxumWsMessage::Ping(data) => send_client_message(client_socket, AxumWsMessage::Pong(data))
            .await
            .map(|()| RelayDisposition::Continue)
            .unwrap_or(RelayDisposition::Close),
        AxumWsMessage::Pong(_) => RelayDisposition::Continue,
        AxumWsMessage::Close(_) => {
            if let Some(upstream) = bound.upstream.as_mut() {
                // Do not forward an untrusted client close reason across the
                // provider trust boundary. A neutral close is sufficient to
                // tear down the bound upstream transport.
                close_upstream_socket(upstream, None).await;
            }
            RelayDisposition::Close
        }
    }
}

/// Revalidates a continuation against the live scheduler while keeping it on
/// the physical provider connection that owns its `previous_response_id`
/// state. This deliberately plans only the pinned provider/endpoint/key: an
/// eligible alternate key is valid for an independent turn, but not for an
/// in-flight response chain.
async fn forward_pinned_continuation(
    bound: &mut BoundResponsesConnection,
    client_socket: &mut WebSocket,
    state: &AppState,
    context: &WebSocketRequestContext,
    planning_parts: http::request::Parts,
    client_event: Value,
    logical_turn_id: String,
    codex_fingerprint_context: CodexFingerprintConvergenceContext,
    turn_control: ResponsesWebSocketTurnControl,
    turn_redaction_session: Option<RedactionSession>,
    plan_usage_permit: Option<aether_runtime::AdmissionPermit>,
    plan_usage_policy_snapshot: Option<PlanUsagePolicySnapshot>,
) -> RelayDisposition {
    let Some(pinned_candidate) =
        ResponsesWebSocketPinnedCandidate::from_decision(&bound.decision_template)
    else {
        warn!(
            event_name = "responses_websocket_continuation_binding_identity_missing",
            log_type = "ops",
            transport = WEBSOCKET_LOG_TRANSPORT,
            websocket = true,
            trace_id = %context.trace_id,
            "gateway cannot revalidate a continuation whose bound candidate identity is incomplete"
        );
        send_gateway_error_with_status(
            client_socket,
            503,
            "responses_continuation_provider_unavailable",
            "The bound provider connection cannot accept this continuation",
        )
        .await;
        return RelayDisposition::Continue;
    };

    // The public protocol allows follow-ups to omit `model`. The planner still
    // needs the effective public model to enumerate the pinned mapping; this
    // injected copy never replaces the already de-duplicated, redacted client
    // event kept for audit and protocol-field restoration.
    let planning_event =
        match pinned_continuation_planning_event(&client_event, bound.client_model.as_str()) {
            Ok(event) => event,
            Err(code) => {
                send_gateway_error(
                    client_socket,
                    code,
                    "response.create.model must be a non-empty string",
                )
                .await;
                return RelayDisposition::Continue;
            }
        };

    let turn_request_id = Uuid::new_v4().to_string();
    let planned = match await_owned_responses_websocket_plan(spawn_owned_responses_websocket_plan(
        state.clone(),
        planning_parts,
        turn_request_id.clone(),
        turn_control.decision.clone(),
        turn_control.auth_snapshot.clone(),
        planning_event,
        None,
        None,
        Some(pinned_candidate),
    ))
    .await
    {
        Ok(Some(decision)) => decision,
        Ok(None) => {
            send_gateway_error_with_status(
                client_socket,
                503,
                "responses_continuation_provider_unavailable",
                "The bound provider is not currently eligible for this continuation",
            )
            .await;
            return RelayDisposition::Continue;
        }
        Err(error) => {
            warn!(
                event_name = "responses_websocket_continuation_revalidation_failed",
                log_type = "ops",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                key_id = ?bound.decision_template.key_id,
                error = ?error,
                "gateway failed to revalidate the bound Responses WebSocket candidate"
            );
            send_gateway_error_with_status(
                client_socket,
                503,
                "responses_continuation_provider_unavailable",
                "Gateway could not revalidate the bound provider",
            )
            .await;
            return RelayDisposition::Continue;
        }
    };
    let OwnedResponsesWebSocketDecision {
        planned,
        planning_parts,
        planned_lease,
    } = planned;
    let adapter = resolve_responses_websocket_adapter(planned.adapter);
    let normalization = planned.normalization;
    let decision = planned.execution;
    let bound_uses_responses_lite = bound.responses_lite_static_config.is_some();
    let planned_uses_responses_lite =
        planned_request_uses_codex_responses_lite(&decision, &normalization);
    if bound_uses_responses_lite != planned_uses_responses_lite
        || (bound_uses_responses_lite
            && !bound
                .body_normalization
                .has_same_responses_lite_static_contract(&normalization))
    {
        planned_lease.release().await;
        send_gateway_error_with_status(
            client_socket,
            409,
            "responses_lite_continuation_contract_changed",
            "The Responses Lite provider contract changed; start a new response without previous_response_id",
        )
        .await;
        return RelayDisposition::Continue;
    }
    let planned_provider_model = provider_model_from_decision(&decision);
    let reuses_bound_upstream = decision_reuses_bound_upstream(bound, adapter, &decision);
    let provider_model_changed =
        planned_provider_model.as_deref() != Some(bound.provider_model.as_str());
    if !reuses_bound_upstream || provider_model_changed {
        let binding_change_fields = if reuses_bound_upstream {
            Vec::new()
        } else {
            decision_bound_upstream_change_fields(bound, adapter, &decision)
        };
        planned_lease.release().await;
        warn!(
            event_name = "responses_websocket_continuation_binding_changed",
            log_type = "ops",
            transport = WEBSOCKET_LOG_TRANSPORT,
            websocket = true,
            trace_id = %context.trace_id,
            key_id = ?decision.key_id,
            binding_change_fields = ?binding_change_fields,
            provider_model_changed,
            "gateway rejected a continuation after the pinned candidate's physical binding changed"
        );
        send_gateway_error_with_status(
            client_socket,
            409,
            "responses_continuation_binding_changed",
            "The provider binding changed; reconnect or start an independent response",
        )
        .await;
        return RelayDisposition::Continue;
    }

    let provider_event =
        match planned_response_create_event(&decision, &normalization, &client_event).and_then(
            |event| {
                serde_json::from_str::<Value>(&event)
                    .map_err(|_| "response_create_serialization_failed")
            },
        ) {
            Ok(event) => event,
            Err(code) => {
                planned_lease.release().await;
                send_gateway_error(
                    client_socket,
                    code,
                    "Gateway could not prepare the response.create event",
                )
                .await;
                return RelayDisposition::Continue;
            }
        };
    let outbound = match serde_json::to_string(&provider_event) {
        Ok(outbound) => outbound,
        Err(_) => {
            planned_lease.release().await;
            send_gateway_error(
                client_socket,
                "response_create_serialization_failed",
                "Gateway could not prepare the response.create event",
            )
            .await;
            return RelayDisposition::Continue;
        }
    };
    let turn_index = bound.next_turn_index;
    let turn_decision = prepare_responses_websocket_turn_decision(
        &decision,
        turn_request_id,
        true,
        &client_event,
        &provider_event,
        &context.trace_id,
        turn_index,
        &logical_turn_id,
        1,
    );
    let mut turn = match begin_responses_websocket_turn_with_planned_lease(
        state,
        &context.trace_id,
        planning_parts,
        &turn_control.decision,
        turn_decision,
        &client_event,
        plan_usage_policy_snapshot.clone(),
        planned_lease,
    )
    .await
    {
        Ok(turn) => turn,
        Err(error) => {
            warn!(
                event_name = "responses_websocket_continuation_turn_start_failed",
                log_type = "ops",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                error = ?error,
                "gateway could not admit a revalidated Responses WebSocket continuation"
            );
            send_responses_websocket_turn_start_error(client_socket, &error).await;
            return RelayDisposition::Continue;
        }
    };

    let Some(upstream) = bound.upstream.as_mut() else {
        queue_turn_finalization(
            bound,
            state,
            turn,
            ResponsesWebSocketTurnOutcome::upstream_send_failed(),
        )
        .await;
        return RelayDisposition::UpstreamError("responses_websocket_send_failed");
    };
    match send_responses_websocket_upstream_message(
        upstream,
        WreqWsMessage::text(outbound),
        plan_usage_permit.as_ref(),
        |request_state| turn.record_upstream_request_state(request_state),
    )
    .await
    {
        Ok(()) => {}
        Err(ResponsesWebSocketUpstreamSendError::PlanUsagePermitLost) => {
            turn.release_plan_usage_cost_before_upstream_send(
                state,
                "continuation_plan_usage_permit_lost",
            )
            .await;
            queue_turn_finalization(
                bound,
                state,
                turn,
                ResponsesWebSocketTurnOutcome::connection_admission_lost(),
            )
            .await;
            return RelayDisposition::PlanUsagePermitLost;
        }
        Err(ResponsesWebSocketUpstreamSendError::Transport(_)) => {
            queue_turn_finalization(
                bound,
                state,
                turn,
                ResponsesWebSocketTurnOutcome::upstream_send_failed(),
            )
            .await;
            return RelayDisposition::UpstreamError("responses_websocket_send_failed");
        }
    }

    turn.set_provider_response_headers(bound.upstream_response_headers.clone());
    if let Some(session) = turn_redaction_session {
        bound.redaction_restorer.register(session);
    }
    bound.adapter = adapter;
    bound.decision_template = decision;
    bound.body_normalization = normalization;
    bound.turn_state.begin(
        LogicalTurn::new(client_event, turn_index, logical_turn_id)
            .with_codex_fingerprint_context(codex_fingerprint_context)
            .with_provider_store(provider_event.get("store") == Some(&Value::Bool(true)))
            .with_turn_control(turn_control)
            .with_plan_usage_permit(plan_usage_permit)
            .with_plan_usage_policy_snapshot(plan_usage_policy_snapshot),
        turn,
    );
    bound.next_turn_index = bound.next_turn_index.saturating_add(1);
    debug!(
        event_name = "responses_websocket_continuation_forwarding",
        log_type = "event",
        transport = WEBSOCKET_LOG_TRANSPORT,
        websocket = true,
        trace_id = %context.trace_id,
        turn_index,
        client_model = %bound.client_model,
        provider_model = %bound.provider_model,
        key_id = ?bound.decision_template.key_id,
        candidate_revalidated = true,
        "gateway forwarded a pinned Responses WebSocket continuation after runtime revalidation"
    );
    RelayDisposition::Continue
}

/// 重新规划一轮 `response.create`（换模型或独立轮）。
///
/// `planning_parts` 与 `client_event` 都由调用方准备：事件已经过请求侧脱敏，
/// Parts 携带这一轮的 `RedactionSessionSlot`，所以 planner 里的候选级脱敏对
/// 已脱敏内容是幂等的 no-op，上游请求体与审计 body 都保持脱敏态。
async fn forward_replanned_response_create(
    bound: &mut BoundResponsesConnection,
    client_socket: &mut WebSocket,
    state: &AppState,
    context: &WebSocketRequestContext,
    planning_parts: http::request::Parts,
    client_event: Value,
    requested_model: String,
    logical_turn_id: String,
    codex_fingerprint_context: CodexFingerprintConvergenceContext,
    turn_control: ResponsesWebSocketTurnControl,
    raw_responses_lite_static_config: ResponsesLiteStaticConfig,
    turn_redaction_session: Option<RedactionSession>,
    plan_usage_permit: Option<aether_runtime::AdmissionPermit>,
    plan_usage_policy_snapshot: Option<PlanUsagePolicySnapshot>,
) -> RelayDisposition {
    let turn_request_id = Uuid::new_v4().to_string();
    let now_unix_secs = current_unix_secs();
    let excluded_key_ids = bound.exhausted_exclusions.key_ids(now_unix_secs);
    let excluded_codex_account_ids = bound.exhausted_exclusions.codex_account_ids(now_unix_secs);
    let excluded_key_ids = (!excluded_key_ids.is_empty()).then_some(excluded_key_ids);
    let excluded_codex_account_ids =
        (!excluded_codex_account_ids.is_empty()).then_some(excluded_codex_account_ids);
    let planned = match await_owned_responses_websocket_plan(spawn_owned_responses_websocket_plan(
        state.clone(),
        planning_parts,
        turn_request_id.clone(),
        turn_control.decision.clone(),
        turn_control.auth_snapshot.clone(),
        client_event.clone(),
        excluded_key_ids,
        excluded_codex_account_ids,
        None,
    ))
    .await
    {
        Ok(Some(decision)) => decision,
        Ok(None) => {
            send_gateway_error_with_status(
                client_socket,
                503,
                "responses_provider_unavailable",
                "No eligible WebSocket-enabled Responses provider is available for the requested model",
            )
            .await;
            return RelayDisposition::Continue;
        }
        Err(error) => {
            warn!(
                event_name = "responses_websocket_followup_model_planning_failed",
                log_type = "ops",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                requested_model = %requested_model,
                error = ?error,
                "gateway failed to re-plan Responses WebSocket follow-up model"
            );
            send_gateway_error_with_status(
                client_socket,
                503,
                "responses_provider_unavailable",
                "Gateway could not prepare the requested model",
            )
            .await;
            return RelayDisposition::Continue;
        }
    };
    let OwnedResponsesWebSocketDecision {
        planned,
        planning_parts,
        planned_lease,
    } = planned;
    let adapter = resolve_responses_websocket_adapter(planned.adapter);
    let normalization = planned.normalization;
    let decision = planned.execution;
    let reuses_bound_upstream = decision_reuses_bound_upstream(bound, adapter, &decision);
    let provider_event =
        match planned_response_create_event(&decision, &normalization, &client_event).and_then(
            |event| {
                serde_json::from_str::<Value>(&event)
                    .map_err(|_| "response_create_serialization_failed")
            },
        ) {
            Ok(event) => event,
            Err(code) => {
                planned_lease.release().await;
                send_gateway_error(
                    client_socket,
                    code,
                    "Gateway could not prepare the requested model",
                )
                .await;
                return RelayDisposition::Continue;
            }
        };
    let turn_index = bound.next_turn_index;
    let turn_decision = prepare_responses_websocket_turn_decision(
        &decision,
        turn_request_id,
        true,
        &client_event,
        &provider_event,
        &context.trace_id,
        turn_index,
        &logical_turn_id,
        1,
    );
    let mut turn = match begin_responses_websocket_turn_with_planned_lease(
        state,
        &context.trace_id,
        planning_parts,
        &turn_control.decision,
        turn_decision,
        &client_event,
        plan_usage_policy_snapshot.clone(),
        planned_lease,
    )
    .await
    {
        Ok(turn) => turn,
        Err(error) => {
            warn!(
                event_name = "responses_websocket_replanned_turn_lifecycle_start_failed",
                log_type = "ops",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                requested_model = %requested_model,
                error = ?error,
                "gateway could not start re-planned WebSocket usage/audit lifecycle"
            );
            send_responses_websocket_turn_start_error(client_socket, &error).await;
            return RelayDisposition::Continue;
        }
    };

    if reuses_bound_upstream {
        let outbound = match serde_json::to_string(&provider_event) {
            Ok(outbound) => outbound,
            Err(_) => {
                queue_turn_finalization(
                    bound,
                    state,
                    turn,
                    ResponsesWebSocketTurnOutcome::upstream_send_failed(),
                )
                .await;
                send_gateway_error(
                    client_socket,
                    "response_create_serialization_failed",
                    "Gateway could not prepare the requested model",
                )
                .await;
                return RelayDisposition::Continue;
            }
        };
        let Some(upstream) = bound.upstream.as_mut() else {
            queue_turn_finalization(
                bound,
                state,
                turn,
                ResponsesWebSocketTurnOutcome::upstream_send_failed(),
            )
            .await;
            return RelayDisposition::UpstreamError("responses_websocket_send_failed");
        };
        match send_responses_websocket_upstream_message(
            upstream,
            WreqWsMessage::text(outbound),
            plan_usage_permit.as_ref(),
            |request_state| turn.record_upstream_request_state(request_state),
        )
        .await
        {
            Ok(()) => {}
            Err(ResponsesWebSocketUpstreamSendError::PlanUsagePermitLost) => {
                turn.release_plan_usage_cost_before_upstream_send(
                    state,
                    "replanned_reuse_plan_usage_permit_lost",
                )
                .await;
                queue_turn_finalization(
                    bound,
                    state,
                    turn,
                    ResponsesWebSocketTurnOutcome::connection_admission_lost(),
                )
                .await;
                return RelayDisposition::PlanUsagePermitLost;
            }
            Err(ResponsesWebSocketUpstreamSendError::Transport(_)) => {
                queue_turn_finalization(
                    bound,
                    state,
                    turn,
                    ResponsesWebSocketTurnOutcome::upstream_send_failed(),
                )
                .await;
                return RelayDisposition::UpstreamError("responses_websocket_send_failed");
            }
        }

        // A response.create without previous_response_id starts a new chain.
        // IDs from the preceding chain must not be accepted merely because
        // the planner can reuse the same physical provider socket.
        bound.continuation_response_ids.clear();
        bound
            .redaction_restorer
            .start_new_chain(turn_redaction_session);
        turn.set_provider_response_headers(bound.upstream_response_headers.clone());
        let provider_model =
            provider_model_from_decision(&decision).unwrap_or_else(|| bound.provider_model.clone());
        let previous_client_model = std::mem::replace(&mut bound.client_model, requested_model);
        let previous_provider_model = std::mem::replace(&mut bound.provider_model, provider_model);
        let uses_responses_lite =
            planned_request_uses_codex_responses_lite(&decision, &normalization);
        bound.decision_template = decision;
        // The re-plan keeps this upstream but resolved a new model, so later
        // continuations must normalize against the new plan, not the old one.
        bound.responses_lite_static_config =
            uses_responses_lite.then_some(raw_responses_lite_static_config);
        bound.body_normalization = normalization;
        bound.turn_state.begin(
            LogicalTurn::new(client_event.clone(), turn_index, logical_turn_id.clone())
                .with_codex_fingerprint_context(codex_fingerprint_context.clone())
                .with_provider_store(provider_event.get("store") == Some(&Value::Bool(true)))
                .with_turn_control(turn_control)
                .with_plan_usage_permit(plan_usage_permit)
                .with_plan_usage_policy_snapshot(plan_usage_policy_snapshot),
            turn,
        );
        bound.next_turn_index = bound.next_turn_index.saturating_add(1);
        debug!(
            event_name = "responses_websocket_followup_model_replanned",
            log_type = "event",
            transport = WEBSOCKET_LOG_TRANSPORT,
            websocket = true,
            trace_id = %context.trace_id,
            turn_index,
            previous_client_model = %previous_client_model,
            client_model = %bound.client_model,
            previous_provider_model = %previous_provider_model,
            provider_model = %bound.provider_model,
            upstream_rebound = false,
            model_replanned = true,
            "gateway re-planned a Responses WebSocket model on the existing upstream"
        );
        return RelayDisposition::Continue;
    }

    let mut replacement = match bind_responses_upstream(
        &decision,
        normalization,
        &client_event,
        adapter,
        plan_usage_permit.as_ref(),
        |request_state| turn.record_upstream_request_state(request_state),
    )
    .await
    {
        Ok(connection) => connection,
        Err(ResponsesWebSocketUpstreamBindError::PlanUsagePermitLost) => {
            turn.release_plan_usage_cost_before_upstream_send(
                state,
                "replanned_rebind_plan_usage_permit_lost",
            )
            .await;
            queue_turn_finalization(
                bound,
                state,
                turn,
                ResponsesWebSocketTurnOutcome::connection_admission_lost(),
            )
            .await;
            return RelayDisposition::PlanUsagePermitLost;
        }
        Err(ResponsesWebSocketUpstreamBindError::Transport(code)) => {
            queue_turn_finalization(
                bound,
                state,
                turn,
                ResponsesWebSocketTurnOutcome::upstream_connect_failed(code),
            )
            .await;
            warn!(
                event_name = "responses_websocket_followup_model_rebind_failed",
                log_type = "ops",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                requested_model = %requested_model,
                error_code = code,
                "gateway failed to rebind Responses WebSocket follow-up model"
            );
            send_gateway_error_with_status(
                client_socket,
                502,
                code,
                "Gateway could not establish the requested model",
            )
            .await;
            return RelayDisposition::Continue;
        }
    };
    if replacement.responses_lite_static_config.is_some() {
        replacement.responses_lite_static_config = Some(raw_responses_lite_static_config);
    }

    turn.set_provider_response_headers(replacement.upstream_response_headers.clone());
    let previous_client_model = bound.client_model.clone();
    let previous_provider_model = bound.provider_model.clone();
    let replacement_upstream = replacement
        .upstream
        .take()
        .expect("newly bound Responses upstream should be present");
    if let Some(mut previous_upstream) = bound.upstream.replace(replacement_upstream) {
        close_upstream_socket(&mut previous_upstream, None).await;
    }
    // Provider connection-local response state cannot survive a physical
    // rebind, and this request starts an independent chain in any case.
    bound.continuation_response_ids.clear();
    bound
        .redaction_restorer
        .start_new_chain(turn_redaction_session);
    bound.adapter = replacement.adapter;
    bound.client_model = replacement.client_model;
    bound.provider_model = replacement.provider_model;
    bound.decision_template = replacement.decision_template;
    bound.body_normalization = replacement.body_normalization;
    bound.responses_lite_static_config = replacement.responses_lite_static_config;
    bound.binding_identity = replacement.binding_identity;
    bound.turn_state.begin(
        LogicalTurn::new(client_event, turn_index, logical_turn_id)
            .with_codex_fingerprint_context(codex_fingerprint_context)
            .with_provider_store(provider_event.get("store") == Some(&Value::Bool(true)))
            .with_turn_control(turn_control)
            .with_plan_usage_permit(plan_usage_permit)
            .with_plan_usage_policy_snapshot(plan_usage_policy_snapshot),
        turn,
    );
    bound.next_turn_index = bound.next_turn_index.saturating_add(1);
    bound.upstream_response_headers = replacement.upstream_response_headers;
    bound.pending_adapter_drain = replacement.pending_adapter_drain;
    debug!(
        event_name = "responses_websocket_followup_model_rebound",
        log_type = "event",
        transport = WEBSOCKET_LOG_TRANSPORT,
        websocket = true,
        trace_id = %context.trace_id,
        turn_index,
        previous_client_model = %previous_client_model,
        requested_model = %requested_model,
        previous_provider_model = %previous_provider_model,
        provider_model = %bound.provider_model,
        upstream_rebound = true,
        model_replanned = true,
        "gateway rebound Responses WebSocket for a follow-up model"
    );
    RelayDisposition::Continue
}

pub(super) async fn consume_response_create_rate_limit(
    state: &AppState,
    decision: &GatewayControlDecision,
    rpm_bypassed: bool,
) -> Result<bool, ()> {
    if rpm_bypassed {
        return Ok(true);
    }
    match state
        .frontdoor_user_rpm()
        .check_and_consume(state, Some(decision))
        .await
        .map_err(|_| ())?
    {
        FrontdoorUserRpmOutcome::Rejected(_) => Ok(false),
        FrontdoorUserRpmOutcome::Allowed | FrontdoorUserRpmOutcome::NotApplicable => Ok(true),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        continuation_constraint, parse_response_create_event, pinned_continuation_planning_event,
        ContinuationConstraint,
    };

    #[test]
    fn invalid_client_text_does_not_poison_the_next_response_create() {
        assert_eq!(
            parse_response_create_event("not-json")
                .expect_err("invalid JSON")
                .code,
            "invalid_response_create"
        );
        assert_eq!(
            parse_response_create_event("[]")
                .expect_err("non-object JSON")
                .code,
            "invalid_response_create"
        );
        assert_eq!(
            parse_response_create_event(r#"{"type":"response.cancel"}"#)
                .expect_err("wrong event type")
                .code,
            "expected_response_create"
        );
        for invalid_previous_response_id in [
            r#"{"type":"response.create","previous_response_id":""}"#,
            r#"{"type":"response.create","previous_response_id":42}"#,
            r#"{"type":"response.create","previous_response_id":{"id":"resp_1"}}"#,
        ] {
            assert_eq!(
                parse_response_create_event(invalid_previous_response_id)
                    .expect_err("invalid previous_response_id")
                    .code,
                "invalid_response_create_previous_response_id"
            );
        }
        let invalid_previous_on_named_lane = parse_response_create_event(
            r#"{"type":"response.create","stream_id":"main","previous_response_id":""}"#,
        )
        .expect_err("invalid previous_response_id must retain a valid lane identity");
        assert_eq!(
            invalid_previous_on_named_lane.code,
            "invalid_response_create_previous_response_id"
        );
        assert_eq!(
            invalid_previous_on_named_lane.stream_id.as_deref(),
            Some("main")
        );
        let named_stream = parse_response_create_event(
            r#"{"type":"response.create","stream_id":"main-lane_1.test"}"#,
        )
        .expect_err("named streams are not implemented");
        assert_eq!(
            named_stream.code,
            "responses_websocket_named_stream_unsupported"
        );
        assert_eq!(named_stream.stream_id.as_deref(), Some("main-lane_1.test"));
        for invalid_stream_id in [
            r#"{"type":"response.create","stream_id":null}"#,
            r#"{"type":"response.create","stream_id":""}"#,
            r#"{"type":"response.create","stream_id":"not/a/lane"}"#,
        ] {
            let error = parse_response_create_event(invalid_stream_id)
                .expect_err("invalid stream_id must be rejected");
            assert_eq!(error.code, "invalid_response_create_stream_id");
            assert_eq!(error.stream_id, None);
        }

        let valid = parse_response_create_event(
            r#"{"type":"response.create","model":"gpt-test","store":true}"#,
        )
        .expect("a valid event after rejected text should still parse");
        assert_eq!(valid["type"], "response.create");
        assert_eq!(valid["store"], true);
    }

    #[test]
    fn continuation_without_an_upstream_never_falls_through_to_replanning() {
        let continuation = json!({
            "type": "response.create",
            "previous_response_id": "resp-previous",
        });

        assert_eq!(
            continuation_constraint(&continuation, "gpt-current", false, true),
            Ok(Some(ContinuationConstraint::UpstreamUnavailable))
        );
        assert_eq!(
            continuation_constraint(&continuation, "gpt-current", true, true),
            Ok(Some(ContinuationConstraint::Pinned))
        );
        assert_eq!(
            continuation_constraint(
                &json!({"type": "response.create", "model": "gpt-current"}),
                "gpt-current",
                false,
                false,
            ),
            Ok(None)
        );
    }

    #[test]
    fn model_changing_continuation_never_falls_through_to_replanning() {
        let continuation = json!({
            "type": "response.create",
            "model": "gpt-other",
            "previous_response_id": "resp-previous",
        });

        assert_eq!(
            continuation_constraint(&continuation, "gpt-current", true, true),
            Ok(Some(ContinuationConstraint::ModelChangeUnsupported))
        );
    }

    #[test]
    fn unknown_same_socket_response_id_never_reaches_the_pinned_provider() {
        let continuation = json!({
            "type": "response.create",
            "model": "gpt-current",
            "previous_response_id": "resp_from_another_principal",
        });

        assert_eq!(
            continuation_constraint(&continuation, "gpt-current", true, false),
            Ok(Some(ContinuationConstraint::UnknownResponseId))
        );
    }

    #[test]
    fn pinned_planning_uses_the_canonical_bound_model() {
        let client_event = json!({
            "type": "response.create",
            "model": "  GPT-CURRENT  ",
            "previous_response_id": "resp-previous",
        });

        let planning = pinned_continuation_planning_event(&client_event, "gpt-current")
            .expect("a same-model continuation should normalize");
        assert_eq!(planning["model"], "gpt-current");
        assert_eq!(client_event["model"], "  GPT-CURRENT  ");
        assert_eq!(planning["previous_response_id"], "resp-previous");
    }
}
