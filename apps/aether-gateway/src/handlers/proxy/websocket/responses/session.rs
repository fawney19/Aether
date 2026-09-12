//! Standard OpenAI Responses WebSocket session engine.
//!
//! An incoming client socket is authenticated at Upgrade time. Its first
//! `response.create` selects a provider through the normal Responses planner.
//! Later turns reuse that upstream while the requested model remains eligible
//! on the selected key. A model change is planned again and keeps the current
//! upstream when the planner resolves to the same target; an independent
//! request may replace it, but a continuation must stay on the original
//! connection and account.

use axum::extract::ws::{Message as AxumWsMessage, WebSocket};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use uuid::Uuid;

use super::adapter::resolve_responses_websocket_adapter;
use super::binding::UpstreamBindingIdentity;
use super::client::consume_response_create_rate_limit;
use super::connection::{relay_bound_connection, wait_for_connection_permit_loss};
use super::continuation::{
    ResponsesWebSocketContinuationRecord, ResponsesWebSocketContinuationRegistry,
};
use super::control::resolve_responses_websocket_turn_control;
use super::lifecycle::{
    await_pending_adapter_observation, await_pending_turn_finalization,
    await_turn_finalization_handle, finalize_active_turn, finalize_unbound_turn,
    responses_websocket_turn_start_close, send_responses_websocket_turn_start_error,
};
use super::ownership::{
    await_owned_responses_websocket_plan, begin_responses_websocket_turn_with_planned_lease,
    spawn_owned_responses_websocket_plan, OwnedResponsesWebSocketDecision,
};
use super::plan_admission::{
    acquire_responses_websocket_plan_admission, responses_websocket_plan_admission_close,
    send_responses_websocket_plan_admission_error,
    terminate_responses_websocket_for_plan_permit_loss,
};
use super::redaction::redact_responses_websocket_client_event_with_reasoning_replay_policy;
use super::relay_policy::{fatal_relay_policy, FatalRelaySignal};
use super::request::{
    build_planning_parts, planned_request_uses_codex_responses_lite, planned_response_create_event,
    prepare_responses_lite_continuation, validate_response_create_previous_response_id,
    validate_response_create_stream_id_support, validated_named_stream_id,
    validated_response_create_model, ResponsesLiteStaticConfig,
};
use super::state::BoundResponsesConnection;
use super::turn::{prepare_responses_websocket_turn_decision, ResponsesWebSocketTurnOutcome};
use super::turn_state::LogicalTurn;
use super::upstream::{
    bind_responses_upstream, close_bound_upstream, ResponsesWebSocketUpstreamBindError,
};

use crate::ai_serving::ResponsesWebSocketPinnedCandidate;
use crate::handlers::proxy::websocket::ingress::{
    WebSocketConnectionLog, WebSocketConnectionLogSpec, WebSocketRequestContext,
};
use crate::handlers::proxy::websocket::session::{
    CLOSE_INTERNAL_ERROR, CLOSE_POLICY_VIOLATION, CLOSE_TRY_AGAIN,
    RESPONSES_WEBSOCKET_SESSION_LIMITS, WEBSOCKET_LOG_TRANSPORT,
};
use crate::handlers::proxy::websocket::transport::{
    close_client_socket, send_gateway_error, send_gateway_error_with_status,
    send_gateway_error_with_stream_id, send_responses_websocket_error_with_param,
};
use crate::privacy::RedactionSession;
use crate::AppState;

const RESPONSES_WEBSOCKET_LOG_TARGET: &str = "aether_gateway::handlers::proxy::responses_ws";
const CONTINUATION_LOOKUP_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);
const RESPONSES_CONNECTION_LOG_SPEC: WebSocketConnectionLogSpec = WebSocketConnectionLogSpec {
    opened_event_name: "responses_websocket_connection_opened",
    closed_event_name: "responses_websocket_connection_closed",
    opened_message: "gateway accepted Responses WebSocket connection",
    closed_message: "gateway closed Responses WebSocket connection",
    execution_path: "responses_websocket_bridge",
    provider_type: "responses",
};

macro_rules! warn {
    ($($arg:tt)*) => {
        tracing::warn!(target: RESPONSES_WEBSOCKET_LOG_TARGET, $($arg)*)
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InitialMessageError {
    TimedOut,
    ClientClosed,
    ClientRead,
    UnsupportedFrame,
    InvalidJson,
    MissingResponseCreate,
    MissingModel,
    InvalidModel,
    InvalidPreviousResponseId,
    InvalidStreamId,
    UnsupportedStreamId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InitialMessageFrameMetadata {
    opcode: &'static str,
    bytes: usize,
}

impl InitialMessageFrameMetadata {
    fn from_message(message: &AxumWsMessage) -> Self {
        match message {
            AxumWsMessage::Text(text) => Self {
                opcode: "text",
                bytes: text.len(),
            },
            AxumWsMessage::Binary(payload) => Self {
                opcode: "binary",
                bytes: payload.len(),
            },
            AxumWsMessage::Ping(payload) => Self {
                opcode: "ping",
                bytes: payload.len(),
            },
            AxumWsMessage::Pong(payload) => Self {
                opcode: "pong",
                bytes: payload.len(),
            },
            AxumWsMessage::Close(frame) => Self {
                opcode: "close",
                // Only the payload length is retained. The untrusted close
                // reason itself must never cross the logging boundary.
                bytes: frame
                    .as_ref()
                    .map_or(0, |frame| 2usize.saturating_add(frame.reason.len())),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InitialMessageFailure {
    error: InitialMessageError,
    last_frame: Option<InitialMessageFrameMetadata>,
    stream_id: Option<String>,
}

impl InitialMessageFailure {
    const fn new(
        error: InitialMessageError,
        last_frame: Option<InitialMessageFrameMetadata>,
    ) -> Self {
        Self {
            error,
            last_frame,
            stream_id: None,
        }
    }

    fn for_event(
        error: InitialMessageError,
        last_frame: Option<InitialMessageFrameMetadata>,
        event: &Value,
    ) -> Self {
        // Preserve a valid lane identity for any request-scoped validation
        // error. Invalid `stream_id` values are rejected by the grammar helper
        // and are never reflected back to the client.
        let stream_id = validated_named_stream_id(event).map(str::to_string);
        Self {
            error,
            last_frame,
            stream_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InitialMessageDiagnostic {
    error_code: &'static str,
    error_kind: &'static str,
    client_message: Option<&'static str>,
    close_code: u16,
    timed_out: bool,
    last_frame_opcode: Option<&'static str>,
    last_frame_bytes: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectionTermination {
    ConnectionLimitReached,
    ConnectionAdmissionLost,
}

impl InitialMessageError {
    const fn code(self) -> &'static str {
        match self {
            Self::TimedOut => "initial_response_create_timeout",
            Self::ClientClosed => "client_closed",
            Self::ClientRead => "client_read_failed",
            Self::UnsupportedFrame => "initial_response_create_must_be_text",
            Self::InvalidJson => "invalid_response_create",
            Self::MissingResponseCreate => "expected_response_create",
            Self::MissingModel => "response_create_model_required",
            Self::InvalidModel => "invalid_response_create_model",
            Self::InvalidPreviousResponseId => "invalid_response_create_previous_response_id",
            Self::InvalidStreamId => "invalid_response_create_stream_id",
            Self::UnsupportedStreamId => "responses_websocket_named_stream_unsupported",
        }
    }

    const fn close_code(self) -> u16 {
        match self {
            Self::TimedOut => CLOSE_TRY_AGAIN,
            Self::ClientClosed => 1000,
            Self::ClientRead | Self::UnsupportedFrame | Self::InvalidJson => CLOSE_POLICY_VIOLATION,
            Self::MissingResponseCreate | Self::MissingModel | Self::InvalidModel => {
                CLOSE_POLICY_VIOLATION
            }
            Self::InvalidPreviousResponseId => CLOSE_POLICY_VIOLATION,
            Self::InvalidStreamId => CLOSE_POLICY_VIOLATION,
            Self::UnsupportedStreamId => CLOSE_POLICY_VIOLATION,
        }
    }

    const fn client_message(self) -> Option<&'static str> {
        match self {
            Self::TimedOut => Some("Timed out waiting for the initial response.create event"),
            Self::ClientClosed => None,
            Self::ClientRead => {
                Some("Failed to read the initial WebSocket event before response.create")
            }
            Self::UnsupportedFrame => {
                Some("The initial response.create event must be sent as a text WebSocket message")
            }
            Self::InvalidJson => {
                Some("The initial WebSocket text message must be a JSON response.create object")
            }
            Self::MissingResponseCreate => {
                Some("The initial WebSocket JSON object must have type response.create")
            }
            Self::MissingModel => Some("The initial response.create event must include a model"),
            Self::InvalidModel => {
                Some("response.create.model must be a non-empty string no longer than 256 bytes")
            }
            Self::InvalidPreviousResponseId => {
                Some("response.create.previous_response_id must be null or a non-empty string")
            }
            Self::InvalidStreamId => Some(
                "response.create.stream_id must be 1-256 ASCII letters, numbers, underscores, hyphens, or periods",
            ),
            Self::UnsupportedStreamId => Some(
                "Aether currently supports only the implicit default WebSocket lane; omit response.create.stream_id",
            ),
        }
    }

    const fn kind(self) -> &'static str {
        match self {
            Self::TimedOut => "timeout",
            Self::ClientClosed => "client_closed",
            Self::ClientRead => "client_read_failed",
            Self::UnsupportedFrame => "unsupported_frame",
            Self::InvalidJson => "invalid_json",
            Self::MissingResponseCreate => "unexpected_event_type",
            Self::MissingModel => "missing_model",
            Self::InvalidModel => "invalid_model",
            Self::InvalidPreviousResponseId => "invalid_previous_response_id",
            Self::InvalidStreamId => "invalid_stream_id",
            Self::UnsupportedStreamId => "unsupported_stream_id",
        }
    }

    const fn timed_out(self) -> bool {
        matches!(self, Self::TimedOut)
    }
}

fn initial_message_diagnostic(failure: &InitialMessageFailure) -> InitialMessageDiagnostic {
    InitialMessageDiagnostic {
        error_code: failure.error.code(),
        error_kind: failure.error.kind(),
        client_message: failure.error.client_message(),
        close_code: failure.error.close_code(),
        timed_out: failure.error.timed_out(),
        last_frame_opcode: failure.last_frame.map(|frame| frame.opcode),
        last_frame_bytes: failure.last_frame.map(|frame| frame.bytes),
    }
}

fn log_initial_message_failure(
    context: &WebSocketRequestContext,
    failure: &InitialMessageFailure,
    upgraded_at: std::time::Instant,
) {
    let diagnostic = initial_message_diagnostic(failure);
    let auth_context = context.decision.auth_context.as_ref();

    // Initial-frame validation intentionally runs before provider planning, so
    // no provider identity exists yet. Keep the usual identity fields in the
    // event schema and say so explicitly instead of guessing from request data.
    warn!(
        event_name = "responses_websocket_initial_event_rejected",
        log_type = "ops",
        transport = WEBSOCKET_LOG_TRANSPORT,
        websocket = true,
        trace_id = %context.trace_id,
        user_id = auth_context.map(|auth| auth.user_id.as_str()).unwrap_or("-"),
        api_key_id = auth_context
            .map(|auth| auth.api_key_id.as_str())
            .unwrap_or("-"),
        provider_selected = false,
        provider_id = "<unplanned>",
        endpoint_id = "<unplanned>",
        key_id = "<unplanned>",
        path = %context.uri.path(),
        route_class = context.decision.route_class.as_deref().unwrap_or("-"),
        route_kind = context.decision.route_kind.as_deref().unwrap_or("-"),
        error_code = diagnostic.error_code,
        error_kind = diagnostic.error_kind,
        close_code = diagnostic.close_code,
        timed_out = diagnostic.timed_out,
        upgrade_to_initial_outcome_ms = upgraded_at.elapsed().as_millis() as u64,
        initial_message_timeout_ms = RESPONSES_WEBSOCKET_SESSION_LIMITS
            .initial_message_timeout
            .as_millis() as u64,
        last_frame_opcode = diagnostic.last_frame_opcode.unwrap_or("none"),
        last_frame_bytes = ?diagnostic.last_frame_bytes,
        "gateway rejected the initial Responses WebSocket event"
    );
}

pub(super) async fn run_responses_websocket(
    mut client_socket: WebSocket,
    state: AppState,
    mut context: WebSocketRequestContext,
) {
    let upgraded_at = std::time::Instant::now();
    // The public connection limit starts when the HTTP Upgrade hands us the
    // socket, not after provider planning and its upstream handshake finish.
    let connection_deadline =
        tokio::time::Instant::now() + RESPONSES_WEBSOCKET_SESSION_LIMITS.max_connection_duration;
    let connection_permit = context.websocket_connection_permit.take();
    let connection_log = WebSocketConnectionLog::new(&context, RESPONSES_CONNECTION_LOG_SPEC);
    connection_log.log_opened();

    let bootstrap_result = supervise_responses_websocket_phase(
        bootstrap_responses_websocket(&mut client_socket, state.clone(), &context, upgraded_at),
        connection_deadline,
        connection_permit.as_ref(),
    )
    .await;

    let mut bound = match bootstrap_result {
        Ok(Some(bound)) => bound,
        Ok(None) => return,
        Err(termination) => {
            // The bootstrap future (and therefore every lease/attempt guard it
            // owned) has been dropped before the permit or client socket is
            // touched here.
            drop(connection_permit);
            close_terminated_bootstrap(&mut client_socket, &context, termination).await;
            return;
        }
    };

    let relay_result = supervise_responses_websocket_phase(
        relay_bound_connection(&mut client_socket, &mut bound, &state, &context),
        connection_deadline,
        connection_permit.as_ref(),
    )
    .await;

    if let Err(termination) = relay_result {
        // The relay future has been dropped before cleanup starts. This makes
        // cancellation safe even when a client-frame branch was awaiting a
        // live auth refresh, redaction, planning, admission, or rebind.
        let outcome = match termination {
            ConnectionTermination::ConnectionLimitReached => {
                ResponsesWebSocketTurnOutcome::connection_limit_reached()
            }
            ConnectionTermination::ConnectionAdmissionLost => {
                ResponsesWebSocketTurnOutcome::connection_admission_lost()
            }
        };
        finalize_active_turn(&mut bound, &state, outcome).await;
        close_bound_upstream(&mut bound).await;
        drop(connection_permit);
        close_terminated_relay(&mut client_socket, &context, termination).await;
    } else {
        drop(connection_permit);
    }
    await_pending_turn_finalization(&mut bound).await;
    await_pending_adapter_observation(&mut bound).await;
}

async fn supervise_responses_websocket_phase<F, T>(
    phase: F,
    connection_deadline: tokio::time::Instant,
    connection_permit: Option<&aether_runtime::AdmissionPermit>,
) -> Result<T, ConnectionTermination>
where
    F: std::future::Future<Output = T>,
{
    tokio::pin!(phase);
    tokio::select! {
        biased;
        _ = tokio::time::sleep_until(connection_deadline) => {
            Err(ConnectionTermination::ConnectionLimitReached)
        }
        _ = wait_for_connection_permit_loss(connection_permit) => {
            Err(ConnectionTermination::ConnectionAdmissionLost)
        }
        output = &mut phase => Ok(output),
    }
}

async fn bootstrap_responses_websocket(
    client_socket: &mut WebSocket,
    state: AppState,
    context: &WebSocketRequestContext,
    upgraded_at: std::time::Instant,
) -> Option<BoundResponsesConnection> {
    let (_first_text, first_event) = match receive_initial_response_create(client_socket).await {
        Ok(value) => value,
        Err(failure) => {
            if let Some(client_message) = failure.error.client_message() {
                log_initial_message_failure(context, &failure, upgraded_at);
                send_gateway_error_with_stream_id(
                    client_socket,
                    failure.error.code(),
                    client_message,
                    failure.stream_id.as_deref(),
                )
                .await;
                close_client_socket(
                    client_socket,
                    failure.error.close_code(),
                    "invalid_initial_event",
                )
                .await;
            }
            return None;
        }
    };

    let initial_previous_response_id = first_event
        .get("previous_response_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);

    // Keep the response-chain identity on the client's raw configuration.
    // Redaction sentinels may rotate between turns, but that must not turn
    // identical plaintext tools/instructions into a synthetic config change.
    let raw_responses_lite_static_config =
        ResponsesLiteStaticConfig::from_response_create(&first_event);

    let first_logical_turn_id = Uuid::now_v7().to_string();
    let mut planning_parts = build_planning_parts(context);
    let first_codex_fingerprint_context =
        crate::ai_serving::codex_context::attach_codex_logical_turn_context(
            &mut planning_parts,
            &first_event,
            &first_logical_turn_id,
        );
    let turn_control = match resolve_responses_websocket_turn_control(
        &state,
        context,
        &planning_parts,
        &first_event,
    )
    .await
    {
        Ok(control) => control,
        Err(error) => {
            warn!(
                event_name = "responses_websocket_initial_turn_control_rejected",
                log_type = "ops",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                error = ?error,
                "gateway rejected a Responses WebSocket initial turn after live policy refresh"
            );
            send_responses_websocket_turn_start_error(client_socket, &error).await;
            let (close_code, close_reason) = responses_websocket_turn_start_close(&error);
            close_client_socket(client_socket, close_code, close_reason).await;
            return None;
        }
    };
    match consume_response_create_rate_limit(
        &state,
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
            close_client_socket(client_socket, CLOSE_TRY_AGAIN, "rate_limit_exceeded").await;
            return None;
        }
        Err(_) => {
            warn!(
                event_name = "responses_websocket_rate_limit_check_failed",
                log_type = "ops",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                "gateway failed to consume WebSocket response rate limit"
            );
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
            return None;
        }
    }

    let plan_usage_admission = match acquire_responses_websocket_plan_admission(
        &state,
        &turn_control.decision,
        &first_logical_turn_id,
    )
    .await
    {
        Ok(admission) => admission,
        Err(error) => {
            warn!(
                event_name = "responses_websocket_initial_plan_usage_rejected",
                log_type = "event",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                logical_turn_id = %first_logical_turn_id,
                error = ?error,
                "gateway rejected the initial Responses WebSocket turn at its subscription plan limit"
            );
            send_responses_websocket_plan_admission_error(client_socket, &error).await;
            let (close_code, close_reason) = responses_websocket_plan_admission_close(&error);
            close_client_socket(client_socket, close_code, close_reason).await;
            return None;
        }
    };
    let crate::plan_usage_policy::PlanUsageAdmission {
        permit: plan_usage_permit,
        policy_snapshot: plan_usage_policy_snapshot,
    } = plan_usage_admission;
    // A cross-socket response chain must be owned by this exact live
    // authenticated principal. Missing, expired, corrupt or unavailable state
    // fails closed; allowing the normal scheduler to choose a provider/key
    // would disclose an opaque response ID to an unrelated account.
    let continuation_record = if let Some(previous_response_id) =
        initial_previous_response_id.as_deref()
    {
        let Some(auth_context) = turn_control.decision.auth_context.as_ref() else {
            reject_initial_previous_response(client_socket).await;
            return None;
        };
        let registry = ResponsesWebSocketContinuationRegistry::new(state.runtime_state.as_ref());
        match tokio::time::timeout(
            CONTINUATION_LOOKUP_TIMEOUT,
            registry.lookup(
                auth_context.user_id.as_str(),
                auth_context.api_key_id.as_str(),
                previous_response_id,
            ),
        )
        .await
        {
            Ok(Ok(Some(record)))
                if record.client_model()
                    == first_event
                        .get("model")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .unwrap_or_default()
                    && !record.has_connection_local_redaction() =>
            {
                Some(record)
            }
            Ok(Ok(Some(record))) => {
                warn!(
                    event_name = "responses_websocket_continuation_registry_rejected",
                    log_type = "ops",
                    transport = WEBSOCKET_LOG_TRANSPORT,
                    websocket = true,
                    trace_id = %context.trace_id,
                    user_id = %auth_context.user_id,
                    api_key_id = %auth_context.api_key_id,
                    provider_id = %record.pinned_candidate().provider_id(),
                    endpoint_id = %record.pinned_candidate().endpoint_id(),
                    key_id = %record.pinned_candidate().key_id(),
                    reason = if record.has_connection_local_redaction() {
                        "connection_local_redaction_state_unavailable"
                    } else {
                        "client_model_mismatch"
                    },
                    "gateway rejected a cross-socket Responses continuation"
                );
                reject_initial_previous_response(client_socket).await;
                return None;
            }
            Ok(Ok(None)) => {
                warn!(
                    event_name = "responses_websocket_continuation_registry_miss",
                    log_type = "ops",
                    transport = WEBSOCKET_LOG_TRANSPORT,
                    websocket = true,
                    trace_id = %context.trace_id,
                    user_id = %auth_context.user_id,
                    api_key_id = %auth_context.api_key_id,
                    reason = "not_found_or_expired",
                    "gateway could not prove ownership of a cross-socket Responses continuation"
                );
                reject_initial_previous_response(client_socket).await;
                return None;
            }
            Ok(Err(error)) => {
                warn!(
                    event_name = "responses_websocket_continuation_registry_lookup_failed",
                    log_type = "ops",
                    transport = WEBSOCKET_LOG_TRANSPORT,
                    websocket = true,
                    trace_id = %context.trace_id,
                    user_id = %auth_context.user_id,
                    api_key_id = %auth_context.api_key_id,
                    reason = error.kind(),
                    "gateway failed closed while looking up a cross-socket Responses continuation"
                );
                reject_initial_previous_response(client_socket).await;
                return None;
            }
            Err(_) => {
                warn!(
                    event_name = "responses_websocket_continuation_registry_lookup_failed",
                    log_type = "ops",
                    transport = WEBSOCKET_LOG_TRANSPORT,
                    websocket = true,
                    trace_id = %context.trace_id,
                    user_id = %auth_context.user_id,
                    api_key_id = %auth_context.api_key_id,
                    reason = "timeout",
                    timeout_ms = CONTINUATION_LOOKUP_TIMEOUT.as_millis() as u64,
                    "gateway timed out looking up a cross-socket Responses continuation"
                );
                reject_initial_previous_response(client_socket).await;
                return None;
            }
        }
    } else {
        None
    };

    // Validate and remove any repeated Lite static prefix against the stored
    // chain identity before PII redaction rotates per-turn sentinels.
    let first_event = match continuation_record
        .as_ref()
        .and_then(ResponsesWebSocketContinuationRecord::responses_lite_static_config)
    {
        Some(stored) => match prepare_responses_lite_continuation(&first_event, stored) {
            Ok(prepared) => prepared,
            Err(_) => {
                warn!(
                    event_name = "responses_websocket_continuation_static_contract_rejected",
                    log_type = "ops",
                    transport = WEBSOCKET_LOG_TRANSPORT,
                    websocket = true,
                    trace_id = %context.trace_id,
                    "gateway rejected changed Responses Lite static configuration on a continuation"
                );
                reject_initial_previous_response(client_socket).await;
                return None;
            }
        },
        None => first_event,
    };

    // 请求侧脱敏必须在规划之前完成，而且这一轮只在这里做一次：planner 会把这份
    // body 写进 upstream 请求体和审计 original_request_body，绑定上游的首条
    // response.create 也从它派生。脱敏失败时直接断开，绝不退回原文发上游。
    let reasoning_replay_policy = continuation_record
        .as_ref()
        .map(ResponsesWebSocketContinuationRecord::reasoning_replay_policy)
        .unwrap_or_default();
    let redacted_first_event =
        redact_responses_websocket_client_event_with_reasoning_replay_policy(
            &state,
            &planning_parts,
            &turn_control.decision,
            &first_event,
            reasoning_replay_policy,
        )
        .await;
    // 首轮的 mask session 要活到响应帧还原，但连接此刻还没绑定，只能先接住，
    // 等 `bind_responses_upstream` 之后登记到连接上。
    let (first_event, first_turn_redaction_session) = match redacted_first_event {
        Ok(Some(redaction)) => (redaction.client_event, Some(redaction.session)),
        Ok(None) => (first_event, None),
        Err(error) => {
            warn!(
                event_name = "responses_websocket_redaction_failed",
                log_type = "ops",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                error = ?error,
                "gateway could not apply chat PII redaction to the initial Responses WebSocket event"
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
            return None;
        }
    };

    let pinned_candidate = match initial_continuation_planning_candidate(
        initial_previous_response_id.is_some(),
        continuation_record
            .as_ref()
            .map(|record| record.pinned_candidate().clone()),
    ) {
        Ok(candidate) => candidate,
        Err(_) => {
            // Keep this invariant next to the planner boundary as a second
            // fail-closed guard: an unproved response ID must never enter the
            // ordinary scheduler and land on an unrelated provider/key.
            reject_initial_previous_response(client_socket).await;
            return None;
        }
    };
    let planned = match await_owned_responses_websocket_plan(spawn_owned_responses_websocket_plan(
        state.clone(),
        planning_parts,
        context.trace_id.clone(),
        turn_control.decision.clone(),
        turn_control.auth_snapshot.clone(),
        first_event.clone(),
        None,
        None,
        pinned_candidate,
    ))
    .await
    {
        Ok(Some(decision)) => decision,
        Ok(None) => {
            if continuation_record.is_some() {
                warn!(
                    event_name = "responses_websocket_continuation_pinned_candidate_unavailable",
                    log_type = "ops",
                    transport = WEBSOCKET_LOG_TRANSPORT,
                    websocket = true,
                    trace_id = %context.trace_id,
                    "gateway could not revalidate the registered Responses continuation binding"
                );
                reject_initial_previous_response(client_socket).await;
                return None;
            }
            send_gateway_error_with_status(
                client_socket,
                503,
                "responses_provider_unavailable",
                "No eligible WebSocket-enabled Responses provider is available",
            )
            .await;
            close_client_socket(
                client_socket,
                CLOSE_TRY_AGAIN,
                "responses_provider_unavailable",
            )
            .await;
            return None;
        }
        Err(_) => {
            warn!(
                event_name = "responses_websocket_planning_failed",
                log_type = "ops",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                "gateway failed to plan Responses WebSocket provider request"
            );
            send_gateway_error_with_status(
                client_socket,
                503,
                "responses_provider_unavailable",
                "Gateway could not prepare a Provider connection",
            )
            .await;
            close_client_socket(
                client_socket,
                CLOSE_INTERNAL_ERROR,
                "responses_planning_failed",
            )
            .await;
            return None;
        }
    };

    let OwnedResponsesWebSocketDecision {
        planned,
        planning_parts,
        planned_lease,
    } = planned;
    let adapter_kind = planned.adapter;
    let adapter = resolve_responses_websocket_adapter(adapter_kind);
    let normalization = planned.normalization;
    let decision = planned.execution;
    if let Some(record) = continuation_record.as_ref() {
        let planned_candidate = ResponsesWebSocketPinnedCandidate::from_decision(&decision);
        let planned_provider_model = decision
            .provider_request_body
            .as_ref()
            .and_then(|body| body.get("model"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or_else(|| {
                decision
                    .mapped_model
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
            });
        let planned_binding = UpstreamBindingIdentity::from_decision(adapter, &decision).ok();
        let planned_uses_responses_lite =
            planned_request_uses_codex_responses_lite(&decision, &normalization);
        let matches_record = record.adapter() == adapter_kind
            && planned_candidate.as_ref() == Some(record.pinned_candidate())
            && planned_provider_model == Some(record.provider_model())
            && responses_lite_contract_modes_match(
                record.responses_lite_static_config().is_some(),
                planned_uses_responses_lite,
            )
            && planned_binding
                .as_ref()
                .is_some_and(|binding| record.matches_contract(binding, &normalization));
        if !matches_record {
            planned_lease.release().await;
            warn!(
                event_name = "responses_websocket_continuation_binding_rejected",
                log_type = "ops",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                provider_id = %record.pinned_candidate().provider_id(),
                endpoint_id = %record.pinned_candidate().endpoint_id(),
                key_id = %record.pinned_candidate().key_id(),
                "gateway rejected a cross-socket continuation after pinned planning changed its contract"
            );
            reject_initial_previous_response(client_socket).await;
            return None;
        }
    }
    let first_provider_event =
        match planned_response_create_event(&decision, &normalization, &first_event).and_then(
            |event| {
                serde_json::from_str::<Value>(&event)
                    .map_err(|_| "responses_websocket_request_invalid")
            },
        ) {
            Ok(event) => event,
            Err(code) => {
                planned_lease.release().await;
                warn!(
                    event_name = "responses_websocket_initial_event_normalization_failed",
                    log_type = "ops",
                    transport = WEBSOCKET_LOG_TRANSPORT,
                    websocket = true,
                    trace_id = %context.trace_id,
                    error_code = code,
                    "gateway could not normalize the initial Responses WebSocket event"
                );
                send_gateway_error(
                    client_socket,
                    code,
                    "Gateway could not prepare the Responses response.create event",
                )
                .await;
                close_client_socket(client_socket, CLOSE_POLICY_VIOLATION, code).await;
                return None;
            }
        };
    let first_turn_decision = prepare_responses_websocket_turn_decision(
        &decision,
        context.trace_id.clone(),
        true,
        &first_event,
        &first_provider_event,
        &context.trace_id,
        1,
        &first_logical_turn_id,
        1,
    );
    let mut first_turn = match begin_responses_websocket_turn_with_planned_lease(
        &state,
        &context.trace_id,
        planning_parts,
        &turn_control.decision,
        first_turn_decision,
        &first_event,
        plan_usage_policy_snapshot.clone(),
        planned_lease,
    )
    .await
    {
        Ok(turn) => turn,
        Err(error) => {
            warn!(
                event_name = "responses_websocket_turn_lifecycle_start_failed",
                log_type = "ops",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                error = ?error,
                "gateway could not start Responses WebSocket usage/audit lifecycle"
            );
            send_responses_websocket_turn_start_error(client_socket, &error).await;
            let (close_code, close_reason) = responses_websocket_turn_start_close(&error);
            close_client_socket(client_socket, close_code, close_reason).await;
            return None;
        }
    };

    let mut bound = match bind_responses_upstream(
        &decision,
        normalization,
        &first_event,
        adapter,
        plan_usage_permit.as_ref(),
        |state| first_turn.record_upstream_request_state(state),
    )
    .await
    {
        Ok(connection) => connection,
        Err(ResponsesWebSocketUpstreamBindError::PlanUsagePermitLost) => {
            first_turn
                .release_plan_usage_cost_before_upstream_send(
                    &state,
                    "initial_plan_usage_permit_lost",
                )
                .await;
            let finalizer = finalize_unbound_turn(
                state.clone(),
                first_turn,
                ResponsesWebSocketTurnOutcome::connection_admission_lost(),
            );
            warn!(
                event_name = "responses_websocket_initial_plan_usage_concurrency_lost",
                log_type = "ops",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                "gateway stopped the initial Responses WebSocket turn before its upstream send after the subscription plan concurrency lease became unhealthy"
            );
            terminate_responses_websocket_for_plan_permit_loss(client_socket).await;
            await_turn_finalization_handle(finalizer).await;
            return None;
        }
        Err(ResponsesWebSocketUpstreamBindError::Transport(code)) => {
            let finalizer = finalize_unbound_turn(
                state.clone(),
                first_turn,
                ResponsesWebSocketTurnOutcome::upstream_connect_failed(code),
            );
            warn!(
                event_name = "responses_websocket_upstream_connect_failed",
                log_type = "ops",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                error_code = code,
                "gateway failed to establish Responses WebSocket upstream"
            );
            send_gateway_error_with_status(
                client_socket,
                502,
                code,
                "Gateway could not establish the Provider connection",
            )
            .await;
            close_client_socket(client_socket, CLOSE_TRY_AGAIN, code).await;
            await_turn_finalization_handle(finalizer).await;
            return None;
        }
    };
    if bound.responses_lite_static_config.is_some() {
        bound.responses_lite_static_config = continuation_record
            .as_ref()
            .and_then(ResponsesWebSocketContinuationRecord::responses_lite_static_config)
            .cloned()
            .or(Some(raw_responses_lite_static_config));
    }
    if let Some(previous_response_id) = initial_previous_response_id.as_deref() {
        // Reaching this point means the principal-scoped registry record and
        // the newly planned physical binding were both proved above. Keep the
        // parent as persisted ownership: the new physical socket may hydrate
        // it, but a failed attempt can evict only its connection-local copy.
        bound
            .continuation_response_ids
            .remember_persisted(previous_response_id);
    }
    first_turn.set_provider_response_headers(bound.upstream_response_headers.clone());
    if let Some(session) = first_turn_redaction_session {
        register_initial_redaction_session(&mut bound, session);
    }
    bound.turn_state.begin(
        LogicalTurn::new(first_event, 1, first_logical_turn_id)
            .with_codex_fingerprint_context(first_codex_fingerprint_context)
            .with_provider_store(first_provider_event.get("store") == Some(&Value::Bool(true)))
            .with_turn_control(turn_control)
            .with_plan_usage_permit(plan_usage_permit)
            .with_plan_usage_policy_snapshot(plan_usage_policy_snapshot),
        first_turn,
    );

    Some(bound)
}

fn register_initial_redaction_session(
    bound: &mut BoundResponsesConnection,
    mut session: RedactionSession,
) {
    session.set_reasoning_replay_policy(bound.body_normalization.reasoning_replay_policy());
    bound.redaction_restorer.register(session);
}

fn responses_lite_contract_modes_match(
    stored_chain_uses_responses_lite: bool,
    planned_request_uses_responses_lite: bool,
) -> bool {
    stored_chain_uses_responses_lite == planned_request_uses_responses_lite
}

fn initial_continuation_planning_candidate(
    has_previous_response_id: bool,
    registered_candidate: Option<ResponsesWebSocketPinnedCandidate>,
) -> Result<Option<ResponsesWebSocketPinnedCandidate>, &'static str> {
    match (has_previous_response_id, registered_candidate) {
        (false, None) => Ok(None),
        (true, Some(candidate)) => Ok(Some(candidate)),
        // A registry miss/corruption and an impossible stray record both fail
        // closed. Neither state is allowed to turn into an unpinned plan.
        (true, None) | (false, Some(_)) => Err("previous_response_not_found"),
    }
}

async fn reject_initial_previous_response(client_socket: &mut WebSocket) {
    send_responses_websocket_error_with_param(
        client_socket,
        400,
        "invalid_request_error",
        "previous_response_not_found",
        "The previous response is unavailable for this authenticated WebSocket connection",
        "previous_response_id",
    )
    .await;
    close_client_socket(
        client_socket,
        CLOSE_POLICY_VIOLATION,
        "previous_response_not_found",
    )
    .await;
}

async fn close_terminated_bootstrap(
    client_socket: &mut WebSocket,
    context: &WebSocketRequestContext,
    termination: ConnectionTermination,
) {
    match termination {
        ConnectionTermination::ConnectionLimitReached => {
            warn!(
                event_name = "responses_websocket_bootstrap_connection_limit_reached",
                log_type = "ops",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                "gateway stopped a Responses WebSocket bootstrap at the absolute connection deadline"
            );
            send_gateway_error_with_status(
                client_socket,
                503,
                "websocket_connection_limit_reached",
                "WebSocket connection duration limit reached; reconnect to continue",
            )
            .await;
            close_client_socket(client_socket, CLOSE_TRY_AGAIN, "connection_limit_reached").await;
        }
        ConnectionTermination::ConnectionAdmissionLost => {
            let policy = fatal_relay_policy(FatalRelaySignal::ConnectionAdmissionLost);
            warn!(
                event_name = "responses_websocket_bootstrap_connection_admission_lost",
                log_type = "ops",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                "gateway stopped a Responses WebSocket bootstrap after its connection admission became unhealthy"
            );
            send_gateway_error_with_status(
                client_socket,
                policy.status_code,
                policy.error_code,
                policy.client_message,
            )
            .await;
            close_client_socket(client_socket, policy.close_code, policy.close_reason).await;
        }
    }
}

async fn close_terminated_relay(
    client_socket: &mut WebSocket,
    context: &WebSocketRequestContext,
    termination: ConnectionTermination,
) {
    match termination {
        ConnectionTermination::ConnectionLimitReached => {
            warn!(
                event_name = "responses_websocket_connection_limit_reached",
                log_type = "ops",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                "gateway stopped a Responses WebSocket relay at the absolute connection deadline"
            );
            send_gateway_error_with_status(
                client_socket,
                503,
                "websocket_connection_limit_reached",
                "WebSocket connection duration limit reached; reconnect to continue",
            )
            .await;
            close_client_socket(client_socket, CLOSE_TRY_AGAIN, "connection_limit_reached").await;
        }
        ConnectionTermination::ConnectionAdmissionLost => {
            let policy = fatal_relay_policy(FatalRelaySignal::ConnectionAdmissionLost);
            warn!(
                event_name = "responses_websocket_connection_admission_lost",
                log_type = "ops",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                "gateway stopped a Responses WebSocket relay after its connection admission became unhealthy"
            );
            send_gateway_error_with_status(
                client_socket,
                policy.status_code,
                policy.error_code,
                policy.client_message,
            )
            .await;
            close_client_socket(client_socket, policy.close_code, policy.close_reason).await;
        }
    }
}

/// 等待客户端发送第一条 response.create 事件。
/// 使用绝对 deadline：从函数入口起计算一次截止时间，Ping/Pong 只会被正常回复，
/// 但不会重置计时器。防止客户端通过周期性 Ping 无限占用 connection permit。
async fn receive_initial_response_create(
    client_socket: &mut WebSocket,
) -> Result<(String, Value), InitialMessageFailure> {
    receive_initial_response_create_with_deadline(
        client_socket,
        RESPONSES_WEBSOCKET_SESSION_LIMITS.initial_message_timeout,
    )
    .await
}

/// 核心循环：在绝对 deadline 内等待客户端发送 response.create。
/// 泛型约束允许测试注入 fake socket，驱动真实逻辑。
///
/// - `deadline_budget`：从调用时刻起的最长等待时间，全循环共享同一截止时刻。
/// - Ping 帧被回复 Pong 但不重置计时器。
/// - Pong / 非法帧 / Close 按协议处理。
async fn receive_initial_response_create_with_deadline<S>(
    socket: &mut S,
    deadline_budget: std::time::Duration,
) -> Result<(String, Value), InitialMessageFailure>
where
    S: futures_util::Stream<Item = Result<AxumWsMessage, axum::Error>>
        + futures_util::Sink<AxumWsMessage, Error = axum::Error>
        + Unpin,
{
    use futures_util::{SinkExt as _, StreamExt as _};

    // 绝对 deadline：入口计算一次，后续所有迭代共享，Ping/Pong 不会重启
    let deadline = tokio::time::Instant::now() + deadline_budget;
    let mut last_frame = None;
    loop {
        let message = tokio::time::timeout_at(deadline, socket.next())
            .await
            .map_err(|_| InitialMessageFailure::new(InitialMessageError::TimedOut, last_frame))?;
        let Some(message) = message else {
            return Err(InitialMessageFailure::new(
                InitialMessageError::ClientClosed,
                last_frame,
            ));
        };
        let message = message
            .map_err(|_| InitialMessageFailure::new(InitialMessageError::ClientRead, last_frame))?;
        last_frame = Some(InitialMessageFrameMetadata::from_message(&message));
        match message {
            AxumWsMessage::Ping(payload) => {
                tokio::time::timeout_at(deadline, socket.send(AxumWsMessage::Pong(payload)))
                    .await
                    .map_err(|_| {
                        InitialMessageFailure::new(InitialMessageError::TimedOut, last_frame)
                    })?
                    .map_err(|_| {
                        InitialMessageFailure::new(InitialMessageError::ClientRead, last_frame)
                    })?;
            }
            AxumWsMessage::Pong(_) => {}
            AxumWsMessage::Close(_) => {
                return Err(InitialMessageFailure::new(
                    InitialMessageError::ClientClosed,
                    last_frame,
                ));
            }
            AxumWsMessage::Binary(_) => {
                return Err(InitialMessageFailure::new(
                    InitialMessageError::UnsupportedFrame,
                    last_frame,
                ));
            }
            AxumWsMessage::Text(text) => {
                let text = text.to_string();
                let event: Value = serde_json::from_str(&text).map_err(|_| {
                    InitialMessageFailure::new(InitialMessageError::InvalidJson, last_frame)
                })?;
                validate_initial_response_create(&event)
                    .map_err(|error| InitialMessageFailure::for_event(error, last_frame, &event))?;
                return Ok((text, event));
            }
        }
    }
}

fn validate_initial_response_create(event: &Value) -> Result<(), InitialMessageError> {
    let object = event.as_object().ok_or(InitialMessageError::InvalidJson)?;
    if object.get("type").and_then(Value::as_str) != Some("response.create") {
        return Err(InitialMessageError::MissingResponseCreate);
    }
    let model = object
        .get("model")
        .ok_or(InitialMessageError::MissingModel)?;
    validated_response_create_model(model).map_err(|_| InitialMessageError::InvalidModel)?;
    validate_response_create_previous_response_id(event)
        .map_err(|_| InitialMessageError::InvalidPreviousResponseId)?;
    match validate_response_create_stream_id_support(event) {
        Ok(()) => {}
        Err("invalid_response_create_stream_id") => {
            return Err(InitialMessageError::InvalidStreamId);
        }
        Err(_) => return Err(InitialMessageError::UnsupportedStreamId),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use super::super::adapter::{
        resolve_responses_websocket_adapter, ResponsesWebSocketDrainDirective,
    };
    use super::super::binding::UpstreamBindingIdentity;
    use super::super::client::adapter_drain_ready;
    use super::super::quota::{
        is_usage_limit_error_event, observe_active_response_rebind_safety,
        record_exhausted_bound_key,
    };
    use super::super::redaction::ResponsesWebSocketRedactionRestorer;
    use super::super::request::{
        changed_followup_response_create_model, normalize_followup_response_create,
        planned_response_create_event, response_create_model_or_current,
    };
    use super::super::state::{BoundResponsesConnection, ExhaustedResponsesWebSocketExclusions};
    use super::super::turn::{
        ResponsesWebSocketTurnDeadline, ResponsesWebSocketTurnObservation,
        ResponsesWebSocketTurnOutcome, ResponsesWebSocketTurnTimeoutPhase,
    };
    use super::super::turn_state::{LogicalTurn, ResponsesTurnState};
    use super::super::upstream::bind_responses_upstream;
    use crate::ai_serving::{
        AiExecutionDecision, OpenAiResponsesReasoningReplayPolicy,
        ResponsesWebSocketBodyNormalization,
    };
    use crate::handlers::proxy::websocket::session::wait_for_optional_deadline;
    use crate::handlers::proxy::websocket::transport::{
        websocket_handshake_headers, websocket_timeouts, websocket_upstream_url,
    };
    use crate::privacy::{RedactionSession, RedactionSessionConfig};
    use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
    use axum::extract::State;
    use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
    use axum::http::HeaderMap;
    use axum::response::IntoResponse;
    use axum::routing::get;
    use axum::Router;
    use futures_util::{SinkExt, StreamExt};
    use serde_json::json;
    use tokio::sync::{oneshot, Mutex};

    #[derive(Default)]
    struct MockState {
        observed: Mutex<Option<oneshot::Sender<ObservedInitialEvent>>>,
    }

    struct ObservedInitialEvent {
        authorization_present: bool,
        account_header_present: bool,
        event: serde_json::Value,
    }

    struct BootstrapDropProbe(Arc<AtomicUsize>);

    impl Drop for BootstrapDropProbe {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct TestAdmissionHealth(Arc<AtomicBool>);

    impl aether_runtime::AdmissionPermitHealth for TestAdmissionHealth {
        fn is_healthy(&self) -> bool {
            self.0.load(Ordering::Acquire)
        }
    }

    #[test]
    fn initial_message_error_diagnostics_are_stable() {
        use super::{initial_message_diagnostic, InitialMessageError, InitialMessageFailure};

        let cases = [
            (
                InitialMessageError::TimedOut,
                "initial_response_create_timeout",
                "timeout",
                Some("Timed out waiting for the initial response.create event"),
                1013,
                true,
            ),
            (
                InitialMessageError::ClientClosed,
                "client_closed",
                "client_closed",
                None,
                1000,
                false,
            ),
            (
                InitialMessageError::ClientRead,
                "client_read_failed",
                "client_read_failed",
                Some("Failed to read the initial WebSocket event before response.create"),
                1008,
                false,
            ),
            (
                InitialMessageError::UnsupportedFrame,
                "initial_response_create_must_be_text",
                "unsupported_frame",
                Some("The initial response.create event must be sent as a text WebSocket message"),
                1008,
                false,
            ),
            (
                InitialMessageError::InvalidJson,
                "invalid_response_create",
                "invalid_json",
                Some("The initial WebSocket text message must be a JSON response.create object"),
                1008,
                false,
            ),
            (
                InitialMessageError::MissingResponseCreate,
                "expected_response_create",
                "unexpected_event_type",
                Some("The initial WebSocket JSON object must have type response.create"),
                1008,
                false,
            ),
            (
                InitialMessageError::MissingModel,
                "response_create_model_required",
                "missing_model",
                Some("The initial response.create event must include a model"),
                1008,
                false,
            ),
            (
                InitialMessageError::InvalidModel,
                "invalid_response_create_model",
                "invalid_model",
                Some("response.create.model must be a non-empty string no longer than 256 bytes"),
                1008,
                false,
            ),
            (
                InitialMessageError::InvalidPreviousResponseId,
                "invalid_response_create_previous_response_id",
                "invalid_previous_response_id",
                Some("response.create.previous_response_id must be null or a non-empty string"),
                1008,
                false,
            ),
            (
                InitialMessageError::InvalidStreamId,
                "invalid_response_create_stream_id",
                "invalid_stream_id",
                Some(
                    "response.create.stream_id must be 1-256 ASCII letters, numbers, underscores, hyphens, or periods",
                ),
                1008,
                false,
            ),
            (
                InitialMessageError::UnsupportedStreamId,
                "responses_websocket_named_stream_unsupported",
                "unsupported_stream_id",
                Some(
                    "Aether currently supports only the implicit default WebSocket lane; omit response.create.stream_id",
                ),
                1008,
                false,
            ),
        ];

        for (error, error_code, error_kind, client_message, close_code, timed_out) in cases {
            let diagnostic = initial_message_diagnostic(&InitialMessageFailure::new(error, None));
            assert_eq!(diagnostic.error_code, error_code);
            assert_eq!(diagnostic.error_kind, error_kind);
            assert_eq!(diagnostic.client_message, client_message);
            assert_eq!(diagnostic.close_code, close_code);
            assert_eq!(diagnostic.timed_out, timed_out);
            assert_eq!(diagnostic.last_frame_opcode, None);
            assert_eq!(diagnostic.last_frame_bytes, None);
        }
    }

    #[test]
    fn initial_message_diagnostic_retains_only_safe_frame_shape() {
        use super::{
            initial_message_diagnostic, InitialMessageError, InitialMessageFailure,
            InitialMessageFrameMetadata,
        };

        let secret_body = r#"{"type":"not-response.create","token":"must-not-log"}"#;
        let frame = Message::Text(secret_body.to_string().into());
        let metadata = InitialMessageFrameMetadata::from_message(&frame);
        let diagnostic = initial_message_diagnostic(&InitialMessageFailure::new(
            InitialMessageError::MissingResponseCreate,
            Some(metadata),
        ));

        assert_eq!(diagnostic.last_frame_opcode, Some("text"));
        assert_eq!(diagnostic.last_frame_bytes, Some(secret_body.len()));
        assert!(!format!("{diagnostic:?}").contains("must-not-log"));
    }

    #[tokio::test]
    async fn phase_supervisor_drops_work_at_the_absolute_connection_deadline() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let task_dropped = Arc::clone(&dropped);
        let bootstrap = async move {
            let _probe = BootstrapDropProbe(task_dropped);
            std::future::pending::<()>().await;
        };

        let result = tokio::time::timeout(
            Duration::from_secs(1),
            super::supervise_responses_websocket_phase(
                bootstrap,
                tokio::time::Instant::now() + Duration::from_millis(20),
                None,
            ),
        )
        .await
        .expect("bootstrap supervisor should honor its absolute deadline");

        assert_eq!(
            result,
            Err(super::ConnectionTermination::ConnectionLimitReached)
        );
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn phase_supervisor_drops_work_when_connection_admission_is_lost() {
        let health = Arc::new(AtomicBool::new(false));
        let permit = aether_runtime::AdmissionPermit::from_parts(
            None,
            Some(TestAdmissionHealth(Arc::clone(&health))),
        )
        .expect("distributed test health should create a permit");
        let dropped = Arc::new(AtomicUsize::new(0));
        let task_dropped = Arc::clone(&dropped);
        let bootstrap = async move {
            let _probe = BootstrapDropProbe(task_dropped);
            std::future::pending::<()>().await;
        };

        let result = tokio::time::timeout(
            Duration::from_secs(1),
            super::supervise_responses_websocket_phase(
                bootstrap,
                tokio::time::Instant::now() + Duration::from_secs(30),
                Some(&permit),
            ),
        )
        .await
        .expect("unhealthy connection admission should stop bootstrap");

        assert_eq!(
            result,
            Err(super::ConnectionTermination::ConnectionAdmissionLost)
        );
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn adapter_drain_waits_for_an_active_turn_terminal_event() {
        let directive = Some(ResponsesWebSocketDrainDirective {
            error_code: "adapter_draining",
            retry_current_turn: false,
            retry_exclusion_until_unix_secs: None,
        });
        assert!(!adapter_drain_ready(directive, true, None, false));
        assert!(adapter_drain_ready(
            directive,
            true,
            Some(ResponsesWebSocketTurnObservation::Terminal(
                ResponsesWebSocketTurnOutcome::upstream_closed()
            )),
            false,
        ));
        assert!(!adapter_drain_ready(None, false, None, false));
        assert!(adapter_drain_ready(directive, false, None, false));
        assert!(adapter_drain_ready(directive, true, None, true));
    }

    #[test]
    fn exhausted_key_and_account_exclusions_expire_at_the_reported_reset_or_fallback() {
        let mut exclusions = ExhaustedResponsesWebSocketExclusions::default();

        assert_eq!(
            exclusions.exclude(
                "key-1".to_string(),
                Some("account-1".to_string()),
                Some(1_050),
                1_000,
            ),
            1_050
        );
        assert!(exclusions.key_ids(1_049).contains("key-1"));
        assert!(exclusions.codex_account_ids(1_049).contains("account-1"));
        assert!(!exclusions.key_ids(1_050).contains("key-1"));
        assert!(!exclusions.codex_account_ids(1_050).contains("account-1"));

        assert_eq!(
            exclusions.exclude("key-2".to_string(), None, None, 2_000),
            2_300
        );
        assert!(exclusions.key_ids(2_299).contains("key-2"));
        assert!(!exclusions.key_ids(2_300).contains("key-2"));

        assert_eq!(
            exclusions.exclude("key-3".to_string(), None, Some(3_100), 3_000),
            3_100
        );
        assert_eq!(
            exclusions.exclude("key-3".to_string(), None, Some(3_050), 3_001),
            3_100
        );
    }

    #[test]
    fn exhausted_codex_binding_excludes_the_account_before_retry_planning() {
        let mut bound = sample_bound_for_rebind_safety();
        bound.decision_template.provider_type = Some("codex".to_string());
        bound.decision_template.key_id = Some("key-codex".to_string());
        bound.decision_template.provider_request_headers.insert(
            "ChatGPT-Account-ID".to_string(),
            "account-codex".to_string(),
        );

        // The exclusion deadline is evaluated against the wall clock, so a
        // provider reset time only survives if it is still in the future.
        let reset_at = crate::clock::current_unix_secs() + 600;

        assert_eq!(
            record_exhausted_bound_key(&mut bound, Some(reset_at)),
            Some(("key-codex".to_string(), reset_at))
        );
        assert!(bound
            .exhausted_exclusions
            .codex_account_ids(reset_at - 1)
            .contains("account-codex"));
        assert!(!bound
            .exhausted_exclusions
            .codex_account_ids(reset_at)
            .contains("account-codex"));
    }

    #[test]
    fn maps_http_responses_url_to_websocket_url_without_losing_path_or_query() {
        let url = websocket_upstream_url(
            "https://example.test/v1/responses?x=1",
            "responses_upstream_url_invalid",
        )
        .expect("URL should convert");
        assert_eq!(url.as_str(), "wss://example.test/v1/responses?x=1");
    }

    #[test]
    fn rejects_embedded_upstream_credentials() {
        assert!(websocket_upstream_url(
            "https://token@example.test/responses",
            "responses_upstream_url_invalid",
        )
        .is_err());
    }

    #[test]
    fn strips_http_entity_headers_from_websocket_handshake() {
        let headers = websocket_handshake_headers(
            &BTreeMap::from([
                (
                    "authorization".to_string(),
                    "Bearer provider-token".to_string(),
                ),
                ("chatgpt-account-id".to_string(), "account-id".to_string()),
                ("content-type".to_string(), "application/json".to_string()),
            ]),
            "responses_websocket_headers_invalid",
        )
        .expect("headers should build");
        assert!(headers.contains_key(AUTHORIZATION));
        assert!(!headers.contains_key(CONTENT_TYPE));
    }

    #[test]
    fn planned_event_uses_mapped_model_and_removes_http_stream_fields() {
        let mut decision = sample_decision();
        decision.provider_request_body = Some(json!({
            "model": "provider-model",
            "input": "hello",
            "stream": true,
            "background": true,
        }));
        let normalization = ResponsesWebSocketBodyNormalization::for_tests("provider-model");
        let event = planned_response_create_event(
            &decision,
            &normalization,
            &json!({
                "type": "response.create",
                "model": "public-model",
                "previous_response_id": "resp-previous",
                "generate": false,
            }),
        )
        .expect("event should serialize");
        let event: serde_json::Value = serde_json::from_str(&event).expect("event JSON");
        assert_eq!(event["type"], "response.create");
        assert_eq!(event["model"], "provider-model");
        assert_eq!(event["previous_response_id"], "resp-previous");
        assert_eq!(event["generate"], false);
        assert!(event.get("stream").is_none());
        assert!(event.get("background").is_none());
    }

    #[test]
    fn only_an_actual_usage_limit_error_requests_transparent_retry() {
        assert!(is_usage_limit_error_event(&json!({
            "type": "error",
            "error": {"type": "usage_limit_reached"},
            "status_code": 429,
        })));
        assert!(!is_usage_limit_error_event(&json!({
            "type": "codex.rate_limits",
            "rate_limits": {"limit_reached": true},
        })));
        assert!(!is_usage_limit_error_event(&json!({
            "type": "response.completed",
            "response": {"id": "resp-completed"},
        })));
    }

    #[test]
    fn followup_rewrites_the_provider_model_and_removes_http_stream_fields() {
        let event = json!({
            "type": "response.create",
            "model": "public-model",
            "stream": true,
            "background": true,
        });
        let normalized = normalize_followup_response_create(
            &event,
            "provider-model",
            &ResponsesWebSocketBodyNormalization::for_tests("provider-model"),
        )
        .expect("response.create should be normalized");
        let event: serde_json::Value = serde_json::from_str(&normalized).expect("event JSON");
        assert_eq!(event["model"], "provider-model");
        assert!(event.get("stream").is_none());
        assert!(event.get("background").is_none());
    }

    #[test]
    fn followup_model_change_requires_per_turn_replanning() {
        let prewarm = json!({
            "type": "response.create",
            "model": "gpt-5.6-sol",
            "generate": false,
        });
        let turn = json!({
            "type": "response.create",
            "model": "gpt-5.6-terra",
            "input": [{"role": "user", "content": "hello"}],
        });

        assert_eq!(
            changed_followup_response_create_model(&prewarm, "gpt-5.6-sol"),
            Ok(None)
        );
        assert_eq!(
            changed_followup_response_create_model(&turn, "gpt-5.6-sol"),
            Ok(Some("gpt-5.6-terra".to_string()))
        );
    }

    #[test]
    fn oversized_model_is_rejected_before_initial_or_followup_planning() {
        let oversized = "m".repeat(257);
        let initial = json!({
            "type": "response.create",
            "model": oversized,
            "input": [],
        });

        assert!(matches!(
            super::validate_initial_response_create(&initial),
            Err(super::InitialMessageError::InvalidModel)
        ));
        assert_eq!(
            changed_followup_response_create_model(&initial, "current-model"),
            Err("invalid_response_create_model")
        );
    }

    #[test]
    fn malformed_initial_previous_response_id_is_rejected_before_planning() {
        for previous_response_id in [json!(""), json!(42), json!({"id": "resp_1"})] {
            let initial = json!({
                "type": "response.create",
                "model": "gpt-5.6-sol",
                "previous_response_id": previous_response_id,
                "input": [],
            });
            assert!(matches!(
                super::validate_initial_response_create(&initial),
                Err(super::InitialMessageError::InvalidPreviousResponseId)
            ));
        }

        let named = json!({
            "type": "response.create",
            "model": "gpt-5.6-sol",
            "stream_id": "main",
            "previous_response_id": "",
            "input": [],
        });
        let failure = super::InitialMessageFailure::for_event(
            super::InitialMessageError::InvalidPreviousResponseId,
            None,
            &named,
        );
        assert_eq!(failure.stream_id.as_deref(), Some("main"));
    }

    #[test]
    fn valid_previous_response_can_start_a_new_socket() {
        let initial = json!({
            "type": "response.create",
            "model": "gpt-5.6-sol",
            "previous_response_id": "resp_existing",
            "input": [{"type": "function_call_output", "call_id": "call_1", "output": "ok"}],
        });
        assert!(super::validate_initial_response_create(&initial).is_ok());

        let null_previous = json!({
            "type": "response.create",
            "model": "gpt-5.6-sol",
            "previous_response_id": null,
            "input": [{"role": "user", "content": "new chain"}],
        });
        assert!(super::validate_initial_response_create(&null_previous).is_ok());
    }

    #[test]
    fn cross_socket_registry_miss_never_falls_through_to_random_provider_planning() {
        let pinned = crate::ai_serving::ResponsesWebSocketPinnedCandidate::new(
            "provider-original",
            "endpoint-original",
            "key-original",
        )
        .expect("valid pinned candidate");

        assert_eq!(
            super::initial_continuation_planning_candidate(false, None),
            Ok(None),
            "a genuinely new response may use the ordinary planner"
        );
        assert_eq!(
            super::initial_continuation_planning_candidate(true, Some(pinned.clone())),
            Ok(Some(pinned)),
            "a proved continuation must retain its exact provider/endpoint/key"
        );
        assert_eq!(
            super::initial_continuation_planning_candidate(true, None),
            Err("previous_response_not_found"),
            "a registry miss must be rejected before an unpinned planner can choose another key"
        );
    }

    #[test]
    fn cross_socket_continuation_rejects_an_effective_lite_mode_change() {
        let mut decision: AiExecutionDecision = serde_json::from_value(json!({
            "action": "local",
            "provider_type": "codex",
            "provider_api_format": "openai:responses",
            "provider_request_headers": {}
        }))
        .expect("minimal Codex decision");
        decision.provider_request_headers.insert(
            crate::ai_serving::CODEX_RESPONSES_LITE_HEADER.to_string(),
            "true".to_string(),
        );
        let normalization = ResponsesWebSocketBodyNormalization::for_tests("gpt-5.6-sol")
            .with_provider_type_for_tests("codex");

        let effective_lite =
            super::planned_request_uses_codex_responses_lite(&decision, &normalization);
        assert!(effective_lite);
        assert!(super::responses_lite_contract_modes_match(
            true,
            effective_lite
        ));

        // A non-null context_management object suppresses the converged Lite
        // contract/header even though the model capability remains enabled.
        // A chain whose stored prefix used Lite must not cross that boundary.
        decision.provider_request_body = Some(json!({
            "model": "gpt-5.6-sol",
            "context_management": {"compact_threshold": 1_000}
        }));
        let effective_lite =
            super::planned_request_uses_codex_responses_lite(&decision, &normalization);
        assert!(!effective_lite);
        assert!(!super::responses_lite_contract_modes_match(
            true,
            effective_lite
        ));
        assert!(super::responses_lite_contract_modes_match(
            false,
            effective_lite
        ));
    }

    #[test]
    fn initial_named_stream_is_rejected_before_planning() {
        let initial = json!({
            "type": "response.create",
            "model": "gpt-5.6-sol",
            "stream_id": "main",
            "input": [],
        });
        assert!(matches!(
            super::validate_initial_response_create(&initial),
            Err(super::InitialMessageError::UnsupportedStreamId)
        ));

        let failure = super::InitialMessageFailure::for_event(
            super::InitialMessageError::UnsupportedStreamId,
            None,
            &initial,
        );
        assert_eq!(failure.stream_id.as_deref(), Some("main"));
    }

    #[test]
    fn malformed_initial_stream_id_is_rejected_before_planning() {
        for stream_id in [json!(null), json!(""), json!("not/a/lane"), json!(42)] {
            let initial = json!({
                "type": "response.create",
                "model": "gpt-5.6-sol",
                "stream_id": stream_id,
                "input": [],
            });
            assert!(matches!(
                super::validate_initial_response_create(&initial),
                Err(super::InitialMessageError::InvalidStreamId)
            ));
            let failure = super::InitialMessageFailure::for_event(
                super::InitialMessageError::InvalidStreamId,
                None,
                &initial,
            );
            assert_eq!(failure.stream_id, None);
        }
    }

    #[test]
    fn model_at_the_identifier_limit_remains_valid() {
        let model = "m".repeat(256);
        let event = json!({
            "type": "response.create",
            "model": model,
            "input": [],
        });

        assert!(super::validate_initial_response_create(&event).is_ok());
        assert_eq!(
            changed_followup_response_create_model(&event, "current-model"),
            Ok(Some(model))
        );
    }

    #[test]
    fn followup_without_a_model_reuses_the_current_connection_model() {
        let event = json!({
            "type": "response.create",
            "input": "continue",
        });

        assert_eq!(
            changed_followup_response_create_model(&event, "gpt-5.6-sol"),
            Ok(None)
        );
    }

    #[test]
    fn detached_followup_inherits_the_current_public_model() {
        let mut event = json!({
            "type": "response.create",
            "input": "start over",
        });

        assert_eq!(
            response_create_model_or_current(&mut event, "gpt-5.6-sol"),
            Ok("gpt-5.6-sol".to_string())
        );
        assert_eq!(event["model"], "gpt-5.6-sol");
    }

    #[test]
    fn quota_retry_requires_an_explicitly_replay_safe_turn() {
        let mut request = LogicalTurn::new(
            json!({"type": "response.create", "model": "gpt-5.6-sol"}),
            2,
            "logical-turn".to_string(),
        );
        assert_eq!(request.quota_retry_block_reason(), None);

        request.mark_retry_unsafe("standard_response_event");
        assert_eq!(
            request.quota_retry_block_reason(),
            Some("standard_response_event")
        );

        let mut retried = LogicalTurn::new(
            json!({"type": "response.create", "model": "gpt-5.6-sol"}),
            2,
            "logical-turn".to_string(),
        );
        retried.retry_attempted = true;
        assert_eq!(
            retried.quota_retry_block_reason(),
            Some("quota_retry_already_attempted")
        );

        let mut client_control = LogicalTurn::new(
            json!({"type": "response.create", "model": "gpt-5.6-sol"}),
            2,
            "logical-turn".to_string(),
        );
        client_control.mark_retry_unsafe("client_control_event");
        assert_eq!(
            client_control.quota_retry_block_reason(),
            Some("client_control_event")
        );

        let continuation = LogicalTurn::new(
            json!({
                "type": "response.create",
                "model": "gpt-5.6-sol",
                "previous_response_id": "resp_previous",
            }),
            2,
            "logical-turn".to_string(),
        );
        assert_eq!(
            continuation.quota_retry_block_reason(),
            Some("previous_response_id")
        );
    }

    #[test]
    fn adapter_safety_contract_controls_transparent_rebind_eligibility() {
        let mut bound = sample_bound_for_rebind_safety();
        observe_active_response_rebind_safety(
            &mut bound,
            &json!({
                "type": "codex.rate_limits",
                "rate_limits": {"allowed": true}
            }),
        );
        assert_eq!(
            bound
                .turn_state
                .logical()
                .and_then(LogicalTurn::quota_retry_block_reason),
            None
        );

        observe_active_response_rebind_safety(&mut bound, &json!({"type": "response.created"}));
        assert_eq!(
            bound
                .turn_state
                .logical()
                .and_then(LogicalTurn::quota_retry_block_reason),
            Some("standard_response_event")
        );

        let mut unknown = sample_bound_for_rebind_safety();
        observe_active_response_rebind_safety(&mut unknown, &json!({"type": "codex.unknown"}));
        assert_eq!(
            unknown
                .turn_state
                .logical()
                .and_then(LogicalTurn::quota_retry_block_reason),
            Some("unrecognized_upstream_event")
        );
    }

    #[test]
    fn websocket_transport_keeps_only_the_connect_timeout() {
        let mut decision = sample_decision();
        decision.timeouts = Some(aether_contracts::ExecutionTimeouts {
            connect_ms: Some(123),
            read_ms: Some(456),
            first_byte_ms: Some(789),
            total_ms: Some(1_000),
            ..aether_contracts::ExecutionTimeouts::default()
        });

        let timeouts = websocket_timeouts(&decision).expect("timeouts should be retained");
        assert_eq!(timeouts.connect_ms, Some(123));
        assert_eq!(timeouts.read_ms, None);
        assert_eq!(timeouts.first_byte_ms, None);
        assert_eq!(timeouts.total_ms, None);
    }

    #[tokio::test]
    async fn expired_turn_deadline_returns_without_waiting_for_socket_io() {
        let deadline = ResponsesWebSocketTurnDeadline {
            phase: ResponsesWebSocketTurnTimeoutPhase::AwaitingFirstEvent,
            deadline: Instant::now() - Duration::from_millis(1),
            timeout: Duration::from_secs(1),
        };

        tokio::time::timeout(
            Duration::from_millis(50),
            wait_for_optional_deadline(Some(deadline.deadline)),
        )
        .await
        .expect("expired deadline should resolve immediately");
    }

    #[tokio::test]
    async fn upstream_binding_uses_provider_headers_and_rewrites_the_first_event() {
        let (upstream_url, observed, server) = spawn_mock_server().await;
        let mut decision = sample_decision();
        decision.upstream_url = Some(upstream_url);
        decision.provider_request_headers = BTreeMap::from([
            (
                "authorization".to_string(),
                "Bearer provider-token".to_string(),
            ),
            ("chatgpt-account-id".to_string(), "account-id".to_string()),
            ("content-type".to_string(), "application/json".to_string()),
        ]);
        decision.provider_request_body = Some(json!({
            "model": "provider-model",
            "input": "hello",
            "stream": true,
            "background": true,
        }));

        let mut bound = bind_responses_upstream(
            &decision,
            ResponsesWebSocketBodyNormalization::for_tests("provider-model"),
            &json!({
                "type": "response.create",
                "model": "public-model",
                "input": "hello",
            }),
            resolve_responses_websocket_adapter(
                crate::orchestration::ResponsesWebSocketAdapter::Standard,
            ),
            None,
            |_| {},
        )
        .await
        .expect("upstream binding should succeed");
        let observed = tokio::time::timeout(Duration::from_secs(2), observed)
            .await
            .expect("mock should observe first event")
            .expect("mock event channel should remain open");
        let response = tokio::time::timeout(
            Duration::from_secs(2),
            bound
                .upstream
                .as_mut()
                .expect("bound upstream should be present")
                .recv(),
        )
        .await
        .expect("mock should send a response event")
        .expect("upstream should remain open")
        .expect("upstream response should be valid");
        server.abort();

        assert!(observed.authorization_present);
        assert!(observed.account_header_present);
        assert_eq!(observed.event["type"], "response.create");
        assert_eq!(observed.event["model"], "provider-model");
        assert!(observed.event.get("stream").is_none());
        assert!(observed.event.get("background").is_none());
        assert!(matches!(response, wreq::ws::message::Message::Text(_)));
    }

    async fn spawn_mock_server() -> (
        String,
        oneshot::Receiver<ObservedInitialEvent>,
        tokio::task::JoinHandle<()>,
    ) {
        let (observed_tx, observed_rx) = oneshot::channel();
        let state = Arc::new(MockState {
            observed: Mutex::new(Some(observed_tx)),
        });
        let app = Router::new()
            .route("/v1/responses", get(mock_websocket))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock listener should bind");
        let address = listener
            .local_addr()
            .expect("mock listener should expose address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("mock server should run");
        });
        (
            format!("http://{address}/v1/responses"),
            observed_rx,
            server,
        )
    }

    async fn mock_websocket(
        ws: WebSocketUpgrade,
        State(state): State<Arc<MockState>>,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        let authorization_present = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("Bearer "));
        let account_header_present = headers.contains_key("chatgpt-account-id");
        ws.on_upgrade(move |socket| async move {
            serve_mock_socket(socket, state, authorization_present, account_header_present).await;
        })
    }

    async fn serve_mock_socket(
        socket: WebSocket,
        state: Arc<MockState>,
        authorization_present: bool,
        account_header_present: bool,
    ) {
        let (mut sender, mut receiver) = socket.split();
        let message = receiver
            .next()
            .await
            .expect("client should send the initial event")
            .expect("initial event should be valid");
        let Message::Text(text) = message else {
            panic!("expected a text response.create event");
        };
        let event = serde_json::from_str(text.as_str()).expect("event should be JSON");
        let _ = sender
            .send(Message::Text(
                json!({"type": "response.created", "response": {"id": "resp-test"}})
                    .to_string()
                    .into(),
            ))
            .await;
        if let Some(observed) = state.observed.lock().await.take() {
            let _ = observed.send(ObservedInitialEvent {
                authorization_present,
                account_header_present,
                event,
            });
        }
    }

    fn sample_decision() -> AiExecutionDecision {
        AiExecutionDecision {
            action: "local".to_string(),
            decision_kind: None,
            execution_strategy: None,
            conversion_mode: None,
            request_id: None,
            candidate_id: None,
            provider_name: None,
            provider_type: Some("custom".to_string()),
            provider_id: None,
            endpoint_id: None,
            key_id: None,
            upstream_base_url: None,
            upstream_url: Some("https://example.test/v1/responses".to_string()),
            provider_request_method: None,
            auth_header: None,
            auth_value: None,
            provider_api_format: Some("openai:responses".to_string()),
            client_api_format: Some("openai:responses".to_string()),
            provider_contract: None,
            client_contract: None,
            model_name: None,
            mapped_model: Some("provider-model".to_string()),
            prompt_cache_key: None,
            extra_headers: BTreeMap::new(),
            provider_request_headers: BTreeMap::new(),
            provider_request_body: None,
            provider_request_body_base64: None,
            content_type: None,
            content_encoding: None,
            request_gzip: None,
            proxy: None,
            transport_profile: None,
            timeouts: None,
            upstream_is_stream: true,
            report_kind: None,
            report_context: None,
            auth_context: None,
        }
    }

    fn sample_bound_for_rebind_safety() -> BoundResponsesConnection {
        let adapter = resolve_responses_websocket_adapter(
            crate::orchestration::ResponsesWebSocketAdapter::Codex,
        );
        let decision = sample_decision();
        let binding_identity = UpstreamBindingIdentity::from_decision(adapter, &decision).unwrap();
        BoundResponsesConnection {
            upstream: None,
            adapter,
            client_model: "gpt-5.6-sol".to_string(),
            provider_model: "gpt-5.6-sol".to_string(),
            decision_template: decision,
            body_normalization: ResponsesWebSocketBodyNormalization::for_tests("gpt-5.6-sol"),
            responses_lite_static_config: None,
            binding_identity,
            continuation_response_ids: Default::default(),
            // Replanning：logical turn 在、attempt 不在。重放安全与配额排除都只看
            // logical turn，所以这些用例不需要真实 socket 或真实 attempt。
            turn_state: ResponsesTurnState::Replanning {
                logical: LogicalTurn::new(
                    json!({"type": "response.create", "model": "gpt-5.6-sol"}),
                    1,
                    "logical-turn".to_string(),
                ),
            },
            next_turn_index: 2,
            upstream_response_headers: BTreeMap::new(),
            pending_adapter_drain: None,
            pending_adapter_observation: None,
            exhausted_exclusions: ExhaustedResponsesWebSocketExclusions::default(),
            pending_turn_finalization: None,
            redaction_restorer: ResponsesWebSocketRedactionRestorer::default(),
        }
    }

    #[test]
    fn initial_deepseek_binding_upgrades_redaction_restore_policy() {
        let mut session = RedactionSession::new(RedactionSessionConfig::new(
            b"initial-deepseek-redaction-test".to_vec(),
            300,
            600,
        ));
        let sentinel = session.redact_text("alice@example.com").text;
        let provider_event = json!({
            "type": "response.completed",
            "response": {
                "output": [{
                    "type": "reasoning",
                    "encrypted_content": "provider-owned-state",
                    "content": [{
                        "type": "reasoning_text",
                        "text": format!("opaque replay {sentinel}")
                    }]
                }]
            }
        });

        let mut ordinary_bound = sample_bound_for_rebind_safety();
        super::register_initial_redaction_session(&mut ordinary_bound, session.clone());
        let restored = ordinary_bound
            .redaction_restorer
            .restore_provider_frame_text(&provider_event)
            .expect("ordinary OpenAI replay policy restoration must not fail")
            .expect("ordinary OpenAI replay policy should restore response text");
        let restored: serde_json::Value =
            serde_json::from_str(&restored).expect("restored provider event should remain JSON");
        assert_eq!(
            restored["response"]["output"][0]["content"][0]["text"],
            "opaque replay alice@example.com"
        );

        let mut deepseek_bound = sample_bound_for_rebind_safety();
        deepseek_bound.body_normalization =
            ResponsesWebSocketBodyNormalization::for_tests("deepseek-reasoner")
                .with_reasoning_replay_policy_for_tests(
                    OpenAiResponsesReasoningReplayPolicy::DeepSeekOpaque,
                );
        super::register_initial_redaction_session(&mut deepseek_bound, session);
        assert!(
            deepseek_bound
                .redaction_restorer
                .restore_provider_frame_text(&provider_event)
                .expect("DeepSeek opaque restoration check must not fail")
                .is_none(),
            "the authenticated DeepSeek binding must keep opaque reasoning state byte-identical"
        );
    }

    /// 用 mpsc 驱动的 FakeSocket，实现 Stream + Sink 两个 trait。
    /// 测试侧通过 tx 注入消息，通过 pong_rx 观察 Pong 回包。
    struct FakeSocket {
        rx: tokio::sync::mpsc::Receiver<axum::extract::ws::Message>,
        pong_tx: tokio::sync::mpsc::UnboundedSender<axum::extract::ws::Message>,
    }

    struct FakeSocketPair {
        tx: tokio::sync::mpsc::Sender<axum::extract::ws::Message>,
        pong_rx: tokio::sync::mpsc::UnboundedReceiver<axum::extract::ws::Message>,
    }

    fn fake_socket() -> (FakeSocket, FakeSocketPair) {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let (pong_tx, pong_rx) = tokio::sync::mpsc::unbounded_channel();
        (FakeSocket { rx, pong_tx }, FakeSocketPair { tx, pong_rx })
    }

    impl futures_util::Stream for FakeSocket {
        type Item = Result<axum::extract::ws::Message, axum::Error>;

        fn poll_next(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Self::Item>> {
            self.rx.poll_recv(cx).map(|opt| opt.map(Ok))
        }
    }

    impl futures_util::Sink<axum::extract::ws::Message> for FakeSocket {
        type Error = axum::Error;

        fn poll_ready(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn start_send(
            self: std::pin::Pin<&mut Self>,
            item: axum::extract::ws::Message,
        ) -> Result<(), Self::Error> {
            let _ = self.pong_tx.send(item);
            Ok(())
        }

        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn poll_close(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    /// Emits one Ping and then permanently backpressures every Pong write.
    /// This models a peer that keeps the TCP connection open but never reads.
    struct StalledPongSocket {
        emitted_ping: bool,
    }

    impl futures_util::Stream for StalledPongSocket {
        type Item = Result<axum::extract::ws::Message, axum::Error>;

        fn poll_next(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Self::Item>> {
            if self.emitted_ping {
                std::task::Poll::Pending
            } else {
                self.emitted_ping = true;
                std::task::Poll::Ready(Some(Ok(axum::extract::ws::Message::Ping(vec![7].into()))))
            }
        }
    }

    impl futures_util::Sink<axum::extract::ws::Message> for StalledPongSocket {
        type Error = axum::Error;

        fn poll_ready(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Pending
        }

        fn start_send(
            self: std::pin::Pin<&mut Self>,
            _item: axum::extract::ws::Message,
        ) -> Result<(), Self::Error> {
            unreachable!("a permanently backpressured sink is never ready")
        }

        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Pending
        }

        fn poll_close(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Pending
        }
    }

    #[tokio::test]
    async fn initial_message_deadline_also_bounds_a_stalled_pong_write() {
        use super::{receive_initial_response_create_with_deadline, InitialMessageError};

        let mut socket = StalledPongSocket {
            emitted_ping: false,
        };
        let result = tokio::time::timeout(
            Duration::from_millis(300),
            receive_initial_response_create_with_deadline(&mut socket, Duration::from_millis(30)),
        )
        .await
        .expect("the initial-message deadline must cancel a stalled Pong write");

        assert!(matches!(
            &result,
            Err(failure) if failure.error == InitialMessageError::TimedOut
        ));
    }

    /// 验证 receive_initial_response_create_with_deadline 的绝对 deadline：
    /// 客户端周期性发送 Ping 帧不会重置计时器，deadline 到期后返回 TimedOut。
    /// 这直接驱动真实的循环逻辑，如果改回每次迭代 timeout(budget, ...) 则会变红。
    ///
    /// 设计思路：deadline_budget = 80ms，Ping 每 30ms 发一次且永不停止。
    /// - 绝对 deadline：~80ms 后函数返回 TimedOut（即使 Ping 仍在到来）。
    /// - 每次迭代 timeout(80ms, ...)：每个 Ping 在 30ms 内到达 < 80ms，函数
    ///   永不超时，500ms 后外层 timeout 判定测试失败。
    #[tokio::test]
    async fn initial_message_times_out_despite_periodic_pings() {
        use super::{receive_initial_response_create_with_deadline, InitialMessageError};

        let (mut fake, pair) = fake_socket();

        let handle = tokio::spawn(async move {
            receive_initial_response_create_with_deadline(&mut fake, Duration::from_millis(80))
                .await
        });

        // 持续发送 Ping，间隔 30ms，永不停止（直到被测函数返回导致 rx drop）
        let ping_task = tokio::spawn(async move {
            let mut i = 0u8;
            loop {
                tokio::time::sleep(Duration::from_millis(30)).await;
                if pair
                    .tx
                    .send(axum::extract::ws::Message::Ping(vec![i].into()))
                    .await
                    .is_err()
                {
                    break;
                }
                i = i.wrapping_add(1);
            }
        });

        // 绝对 deadline 应在 ~80ms 后触发；给 500ms 宽限等待结果。
        let result = tokio::time::timeout(Duration::from_millis(500), handle)
            .await
            .expect("function should return within 500ms (absolute deadline = 80ms)")
            .expect("task should not panic");

        ping_task.abort();

        assert!(
            matches!(
                &result,
                Err(failure) if failure.error == InitialMessageError::TimedOut
            ),
            "expected TimedOut after absolute deadline, got: {result:?}"
        );
    }

    /// 验证 deadline 内收到合法 response.create 时正常返回，Ping 被正确回复 Pong。
    #[tokio::test]
    async fn initial_message_succeeds_within_deadline() {
        use super::{receive_initial_response_create_with_deadline, InitialMessageError};

        let (mut fake, mut pair) = fake_socket();

        let handle = tokio::spawn(async move {
            receive_initial_response_create_with_deadline(&mut fake, Duration::from_secs(5)).await
        });

        // 先发一个 Ping，验证 Pong 回包且不影响后续解析
        pair.tx
            .send(axum::extract::ws::Message::Ping(vec![42].into()))
            .await
            .unwrap();
        let pong = tokio::time::timeout(Duration::from_secs(1), pair.pong_rx.recv())
            .await
            .expect("should receive pong within 1s")
            .expect("pong channel should not close");
        assert!(
            matches!(pong, axum::extract::ws::Message::Pong(ref data) if data.as_ref() == [42]),
            "expected Pong([42]), got: {pong:?}"
        );

        // 发送合法的 response.create
        let event_text = r#"{"type":"response.create","model":"gpt-4o"}"#;
        pair.tx
            .send(axum::extract::ws::Message::Text(
                event_text.to_string().into(),
            ))
            .await
            .unwrap();

        let result = tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("handle should finish within 2s")
            .expect("task should not panic");
        let (text, event) = result.expect("should return Ok");
        assert_eq!(text, event_text);
        assert_eq!(event["type"], "response.create");
        assert_eq!(event["model"], "gpt-4o");
    }
}
