//! Connection-level Responses WebSocket FSM.

use std::time::Duration;

use axum::extract::ws::{Message as AxumWsMessage, WebSocket};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use wreq::ws::message::Message as WreqWsMessage;

use super::adapter::ResponsesWebSocketRelayDirective;
use super::client::{adapter_drain_ready, forward_client_message, RelayDisposition};
use super::continuation::{
    ResponsesWebSocketContinuationRecord, ResponsesWebSocketContinuationRegistry,
};
use super::frame::{encode_opaque_websocket_event, ParsedResponsesWebSocketFrame};
use super::lifecycle::{
    await_pending_adapter_observation, finalize_active_turn, queue_turn_finalization,
    settle_turn_finalization, spawn_bounded_adapter_observation, PreviousAttemptSettled,
};
use super::plan_admission::terminate_responses_websocket_for_plan_permit_loss;
use super::quota::{
    detach_exhausted_upstream, is_usage_limit_error_event, mark_active_response_retry_unsafe,
    observe_active_response_rebind_safety, retry_active_turn_after_quota_exhaustion,
};
use super::relay_policy::{
    classify_quota_relay, fatal_relay_policy, FatalRelaySignal, QuotaRelayAction, QuotaRelayFacts,
};
use super::settlement::settle_signal_for_client_delivery_failure;
use super::state::BoundResponsesConnection;
use super::turn::{
    ResponsesProviderAttempt, ResponsesWebSocketTurnObservation, ResponsesWebSocketTurnOutcome,
};
use super::turn_state::LogicalTurn;
use super::upstream::{close_bound_upstream, receive_optional_upstream};
use crate::handlers::proxy::websocket::ingress::WebSocketRequestContext;
use crate::handlers::proxy::websocket::session::{
    wait_for_optional_deadline, CLOSE_INTERNAL_ERROR, CLOSE_TRY_AGAIN, WEBSOCKET_LOG_TRANSPORT,
};
use crate::handlers::proxy::websocket::transport::{
    close_client_socket, send_client_message, send_gateway_error_with_status,
    send_responses_websocket_error, upstream_message_to_client,
};
use crate::AppState;

const LOG_TARGET: &str = "aether_gateway::handlers::proxy::responses_ws";
const CONTINUATION_REGISTRATION_TIMEOUT: Duration = Duration::from_millis(500);

/// 写客户端 socket 失败时记录的投递失败原因。刻意不说「客户端在终态前断开」：
/// 供应商的终态可能已经到达，只是最后一跳没送出去。
const CLIENT_DELIVERY_FAILED_REASON: &str =
    "gateway could not relay the provider event to the client";

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

pub(super) async fn relay_bound_connection(
    client_socket: &mut WebSocket,
    bound: &mut BoundResponsesConnection,
    state: &AppState,
    context: &WebSocketRequestContext,
) {
    let mut client_connected = true;
    loop {
        if !client_connected && !bound.turn_state.response_in_flight() {
            close_bound_upstream(bound).await;
            break;
        }
        let active_turn_deadline = bound.turn_state.attempt().map(|turn| turn.deadline());
        tokio::select! {
            _ = wait_for_optional_deadline(active_turn_deadline.map(|deadline| deadline.deadline)) => {
                let Some(turn_deadline) = active_turn_deadline else {
                    continue;
                };
                warn!(
                    event_name = "responses_websocket_turn_timeout",
                    log_type = "ops",
                    transport = WEBSOCKET_LOG_TRANSPORT,
                    websocket = true,
                    trace_id = %context.trace_id,
                    timeout_phase = ?turn_deadline.phase,
                    timeout_ms = turn_deadline.timeout.as_millis() as u64,
                    "Responses WebSocket response did not reach its configured deadline"
                );
                finalize_active_turn(bound, state, turn_deadline.phase.outcome()).await;
                send_gateway_error_with_status(
                    client_socket,
                    504,
                    turn_deadline.phase.error_code(),
                    turn_deadline.phase.client_message(),
                ).await;
                close_bound_upstream(bound).await;
                close_client_socket(
                    client_socket,
                    CLOSE_TRY_AGAIN,
                    turn_deadline.phase.error_code(),
                ).await;
                break;
            }
            client_message = client_socket.next(), if client_connected => {
                let Some(client_message) = client_message else {
                    if retain_disconnected_turn(bound) {
                        client_connected = false;
                        continue;
                    }
                    finalize_active_turn(
                        bound,
                        state,
                        ResponsesWebSocketTurnOutcome::client_disconnected(),
                    ).await;
                    close_bound_upstream(bound).await;
                    break;
                };
                let Ok(client_message) = client_message else {
                    if retain_disconnected_turn(bound) {
                        client_connected = false;
                        continue;
                    }
                    warn!(
                        event_name = "responses_websocket_client_receive_failed",
                        log_type = "ops",
                        transport = WEBSOCKET_LOG_TRANSPORT,
                        websocket = true,
                        trace_id = %context.trace_id,
                        "client WebSocket receive failed"
                    );
                    finalize_active_turn(
                        bound,
                        state,
                        ResponsesWebSocketTurnOutcome::client_disconnected(),
                    ).await;
                    close_bound_upstream(bound).await;
                    break;
                };
                if matches!(client_message, AxumWsMessage::Close(_))
                    && retain_disconnected_turn(bound)
                {
                    client_connected = false;
                    continue;
                }
                match Box::pin(forward_client_message(
                    client_message,
                    bound,
                    client_socket,
                    state,
                    context,
                ))
                .await
                {
                    RelayDisposition::Continue => {}
                    RelayDisposition::Close => {
                        finalize_active_turn(
                            bound,
                            state,
                            ResponsesWebSocketTurnOutcome::client_disconnected(),
                        ).await;
                        break;
                    }
                    RelayDisposition::PlanUsagePermitLost => {
                        warn!(
                            event_name = "responses_websocket_plan_usage_concurrency_lost_before_send",
                            log_type = "ops",
                            transport = WEBSOCKET_LOG_TRANSPORT,
                            websocket = true,
                            trace_id = %context.trace_id,
                            "gateway stopped a Responses WebSocket turn before its upstream send after the subscription plan concurrency lease became unhealthy"
                        );
                        close_bound_upstream(bound).await;
                        terminate_responses_websocket_for_plan_permit_loss(client_socket).await;
                        break;
                    }
                    RelayDisposition::UpstreamError(code) => {
                        warn!(
                            event_name = "responses_websocket_upstream_send_failed",
                            log_type = "ops",
                            transport = WEBSOCKET_LOG_TRANSPORT,
                            websocket = true,
                            trace_id = %context.trace_id,
                            error_code = code,
                            "Upstream WebSocket send failed"
                        );
                        finalize_active_turn(
                            bound,
                            state,
                            ResponsesWebSocketTurnOutcome::upstream_send_failed(),
                        ).await;
                        send_gateway_error_with_status(
                            client_socket,
                            502,
                            code,
                            "Gateway could not forward the WebSocket event upstream",
                        ).await;
                        close_bound_upstream(bound).await;
                        close_client_socket(client_socket, CLOSE_INTERNAL_ERROR, code).await;
                        break;
                    }
                }
            }
            upstream_message = receive_optional_upstream(&mut bound.upstream) => {
                let Some(upstream_message) = upstream_message else {
                    finalize_active_turn(
                        bound,
                        state,
                        ResponsesWebSocketTurnOutcome::upstream_closed(),
                    ).await;
                    bound.upstream = None;
                    close_client_socket(client_socket, 1000, "upstream_closed").await;
                    break;
                };
                let Ok(upstream_message) = upstream_message else {
                    warn!(
                        event_name = "responses_websocket_upstream_receive_failed",
                        log_type = "ops",
                        transport = WEBSOCKET_LOG_TRANSPORT,
                        websocket = true,
                        trace_id = %context.trace_id,
                        "Upstream WebSocket receive failed"
                    );
                    finalize_active_turn(
                        bound,
                        state,
                        ResponsesWebSocketTurnOutcome::upstream_receive_failed(),
                    ).await;
                    send_gateway_error_with_status(
                        client_socket,
                        502,
                        "responses_websocket_receive_failed",
                        "Provider connection closed unexpectedly",
                    ).await;
                    bound.upstream = None;
                    close_client_socket(client_socket, CLOSE_INTERNAL_ERROR, "upstream_receive_failed").await;
                    break;
                };
                let parsed_upstream_frame = match &upstream_message {
                    WreqWsMessage::Text(text) => {
                        ParsedResponsesWebSocketFrame::parse(text.as_str()).ok()
                    }
                    _ => None,
                };
                if parsed_upstream_frame
                    .as_ref()
                    .is_some_and(ParsedResponsesWebSocketFrame::carries_stream_id)
                {
                    // The session currently owns only the implicit default
                    // lane. Reject a provider-side named identity before the
                    // adapter, usage observer, continuation cache, PII
                    // restorer, or logical-turn state can attribute an
                    // interleaved event to the sole default-lane attempt.
                    let policy = fatal_relay_policy(
                        FatalRelaySignal::UnexpectedUpstreamStreamId,
                    );
                    let frame = parsed_upstream_frame
                        .as_ref()
                        .expect("the stream-id guard requires a parsed frame");
                    warn!(
                        event_name = "responses_websocket_unexpected_upstream_stream_id",
                        log_type = "ops",
                        transport = WEBSOCKET_LOG_TRANSPORT,
                        websocket = true,
                        trace_id = %context.trace_id,
                        event_type = %frame.event_type_for_log(),
                        frame_bytes = frame.raw_text().len(),
                        chunked = frame.is_chunked(),
                        "gateway rejected a named-lane provider event on a default-lane Responses WebSocket"
                    );
                    finalize_active_turn(
                        bound,
                        state,
                        ResponsesWebSocketTurnOutcome::upstream_receive_failed(),
                    )
                    .await;
                    send_responses_websocket_error(
                        client_socket,
                        policy.status_code,
                        "server_error",
                        policy.error_code,
                        policy.client_message,
                    )
                    .await;
                    close_bound_upstream(bound).await;
                    close_client_socket(
                        client_socket,
                        policy.close_code,
                        policy.close_reason,
                    )
                    .await;
                    break;
                }
                let parsed_upstream_event = parsed_upstream_frame
                    .as_ref()
                    .map(ParsedResponsesWebSocketFrame::event);
                if let WreqWsMessage::Text(text) = &upstream_message {
                    debug!(
                        event_name = "responses_websocket_upstream_event",
                        log_type = "event",
                        transport = WEBSOCKET_LOG_TRANSPORT,
                        websocket = true,
                        trace_id = %context.trace_id,
                        event_type = %parsed_upstream_frame
                            .as_ref()
                            .map(ParsedResponsesWebSocketFrame::event_type_for_log)
                            .unwrap_or_else(|| "invalid_json".to_string()),
                        frame_bytes = text.len(),
                        chunked = parsed_upstream_frame
                            .as_ref()
                            .is_some_and(ParsedResponsesWebSocketFrame::is_chunked),
                        active_turn = bound.turn_state.response_in_flight(),
                        "gateway received Responses WebSocket event"
                    );
                }
                if matches!(&upstream_message, WreqWsMessage::Binary(_)) {
                    mark_active_response_retry_unsafe(bound, "upstream_binary_frame");
                } else if matches!(&upstream_message, WreqWsMessage::Text(_))
                    && parsed_upstream_event.is_none()
                {
                    mark_active_response_retry_unsafe(bound, "invalid_upstream_event");
                }
                if let Some(event) = parsed_upstream_event {
                    observe_active_response_rebind_safety(bound, event);
                    if bound.pending_adapter_drain.is_none()
                        && bound.adapter.observes_upstream_events()
                    {
                        let adapter = bound.adapter;
                        if let Some(observation) = adapter.observe_upstream_event(event) {
                            let directive = observation.drain;
                            await_pending_adapter_observation(bound).await;
                            let state_for_observation = state.clone();
                            let trace_id = context.trace_id.clone();
                            let report_context = bound.decision_template.report_context.clone();
                            bound.pending_adapter_observation = Some(spawn_bounded_adapter_observation(async move {
                                adapter
                                    .persist_upstream_observation(
                                        &state_for_observation,
                                        &trace_id,
                                        report_context.as_ref(),
                                        observation,
                                    )
                                    .await;
                            }));
                            if let Some(directive) = directive {
                                bound.pending_adapter_drain = Some(directive);
                                // A definitive quota signal must be visible to
                                // the next planner before a transparent retry.
                                await_pending_adapter_observation(bound).await;
                            }
                        }
                    }
                }
                let observation = match &upstream_message {
                    WreqWsMessage::Text(text) => {
                        let adapter = bound.adapter;
                        match parsed_upstream_frame.as_ref() {
                            Some(frame) => bound
                                .turn_state
                                .attempt_mut()
                                .and_then(|turn| turn.observe_upstream_frame(frame, adapter)),
                            None => {
                                if let Some(turn) = bound.turn_state.attempt_mut() {
                                    turn.observe_invalid_upstream_text(text.as_str())
                                }
                                else {
                                    None
                                }
                            }
                        }
                    }
                    _ => None,
                };
                if matches!(
                    observation,
                    Some(ResponsesWebSocketTurnObservation::Started)
                        | Some(ResponsesWebSocketTurnObservation::Terminal(_))
                ) {
                    if let Some(turn) = bound.turn_state.attempt_mut() {
                        turn.mark_stream_started(state).await;
                    }
                }
                let terminal_outcome = match observation {
                    Some(ResponsesWebSocketTurnObservation::Terminal(outcome)) => Some(outcome),
                    _ => None,
                };
                if let Some(frame) = parsed_upstream_frame.as_ref() {
                    if let Some(response_id) = evicted_default_lane_continuation_response_id(
                        bound.turn_state.logical(),
                        frame,
                    ) {
                        // A 4xx/5xx continuation terminal evicts the referenced
                        // ID from the provider's implicit default-lane cache.
                        // Do not keep claiming local ownership and replay it.
                        bound
                            .continuation_response_ids
                            .forget_connection_local(response_id);
                    }
                    if let Some(response_id) = connection_local_terminal_response_id(
                        bound.turn_state.logical(),
                        frame,
                    ) {
                        // Remember every successful response on this physical
                        // socket, including store=false responses that exist
                        // only in the provider's connection-local cache.
                        bound
                            .continuation_response_ids
                            .remember_connection_local(response_id);
                    }
                    if let Some(registration) =
                        prepare_persisted_continuation_registration(bound, context, frame)
                    {
                        if let Some(response_id) =
                            register_persisted_continuation_before_terminal_delivery(
                            state,
                            context,
                            registration,
                        )
                            .await
                        {
                            bound
                                .continuation_response_ids
                                .remember_persisted(response_id.as_str());
                        }
                    }
                }
                if matches!(&upstream_message, WreqWsMessage::Text(_))
                    && parsed_upstream_frame.is_none()
                {
                    let policy = fatal_relay_policy(FatalRelaySignal::InvalidUpstreamText);
                    finalize_active_turn(
                        bound,
                        state,
                        terminal_outcome.unwrap_or_else(
                            ResponsesWebSocketTurnOutcome::upstream_receive_failed,
                        ),
                    )
                    .await;
                    send_responses_websocket_error(
                        client_socket,
                        policy.status_code,
                        "server_error",
                        policy.error_code,
                        policy.client_message,
                    )
                    .await;
                    close_bound_upstream(bound).await;
                    close_client_socket(
                        client_socket,
                        policy.close_code,
                        policy.close_reason,
                    )
                    .await;
                    break;
                }
                let is_close = matches!(upstream_message, WreqWsMessage::Close(_));
                let drain_for_adapter = adapter_drain_ready(
                    bound.pending_adapter_drain,
                    bound.turn_state.response_in_flight(),
                    observation,
                    is_close,
                );
                let quota_facts = QuotaRelayFacts {
                    drain_ready: drain_for_adapter,
                    retry_current_turn: bound
                        .pending_adapter_drain
                        .is_some_and(|directive| directive.retry_current_turn)
                        && bound
                            .turn_state
                            .logical()
                            .is_some_and(|turn| turn.quota_retry_block_reason().is_none()),
                    transparent_retry_failed: false,
                    usage_limit_error: parsed_upstream_event.is_some_and(is_usage_limit_error_event),
                    upstream_closed: is_close,
                };
                let mut quota_relay_action = classify_quota_relay(quota_facts);
                if matches!(quota_relay_action, QuotaRelayAction::AttemptTransparentRetry) {
                    // detach_attempt 保留 logical turn：重试是同一轮请求的下一个 attempt。
                    let retry_turn = bound.turn_state.detach_attempt();
                    // 先结算旧 attempt 并等它落地，再规划下一个 attempt。两个理由：
                    //
                    // 1. 规划要读 health / adaptive / pool 状态，而这些正是旧
                    //    attempt 结算时才投射的。普通的新 turn 早就在 client.rs 里
                    //    用 await_pending_turn_finalization 挡住了「基于陈旧状态
                    //    规划」，透明重试这条路径原先漏了这一步。
                    // 2. 旧 attempt 还占着自己的 pool key lease。不先释放，重试就
                    //    可能因为「这把 key 仍被占用」而挑不到本该可用的替代 key，
                    //    或者干脆判成无可用供应商。
                    let settled = match retry_turn {
                        Some(mut turn) => {
                            turn.release_admission().await;
                            settle_turn_finalization(
                                bound,
                                state,
                                turn,
                                terminal_outcome.unwrap_or_else(
                                    ResponsesWebSocketTurnOutcome::upstream_closed,
                                ),
                            )
                            .await
                        }
                        None => Some(PreviousAttemptSettled::nothing_to_settle()),
                    };
                    // Planning and binding a replacement carries the complete
                    // scheduler/provider state machine. Keep that large future
                    // off the relay task's stack; the default Tokio/test worker
                    // stack is otherwise easy to exhaust on this rare branch.
                    if let Some(settled) = settled {
                        match Box::pin(retry_active_turn_after_quota_exhaustion(
                            bound, state, context, settled,
                        ))
                        .await
                        {
                            Ok(true) => continue,
                            Ok(false) => {}
                            Err(()) => {
                                finalize_active_turn(
                                    bound,
                                    state,
                                    ResponsesWebSocketTurnOutcome::connection_admission_lost(),
                                )
                                .await;
                                close_bound_upstream(bound).await;
                                terminate_responses_websocket_for_plan_permit_loss(client_socket)
                                    .await;
                                break;
                            }
                        }
                    }
                    // 重试失败。旧 attempt 已经结算，logical turn 仍停在
                    // Replanning，所以后面分支里的 end() / finalize_active_turn
                    // 只会清掉 logical turn 而不会交出 attempt——不存在重复结算。
                    quota_relay_action = classify_quota_relay(QuotaRelayFacts {
                        retry_current_turn: false,
                        transparent_retry_failed: true,
                        ..quota_facts
                    });
                }
                let detach_after_forward =
                    matches!(quota_relay_action, QuotaRelayAction::ForwardQuotaAndDetach);
                if detach_after_forward && is_close {
                    let directive = bound
                        .pending_adapter_drain
                        .expect("adapter drain state should be present");
                    finalize_active_turn(
                        bound,
                        state,
                        terminal_outcome
                            .unwrap_or_else(ResponsesWebSocketTurnOutcome::provider_quota_exhausted),
                    )
                    .await;
                    send_gateway_error_with_status(
                        client_socket,
                        429,
                        directive.error_code,
                        "Provider connection closed after reporting exhausted quota; send a new response.create to select another Provider connection",
                    )
                    .await;
                    detach_exhausted_upstream(bound, directive, &context.trace_id).await;
                    continue;
                }
                // Standard Responses frames cross the gateway byte-for-byte unless PII
                // restoration has something to replace. Codex may wrap public events with
                // provider-private side-channel chunks; only that explicit envelope is
                // peeled, and each retained event is serialized as a complete opaque Value.
                // Observation and capture continue to consume the redacted event, while the
                // final client hop receives restored text.
                let relay_directive = parsed_upstream_frame
                    .as_ref()
                    .map(|frame| {
                        bound
                            .adapter
                            .relay_directive_for_upstream_event(frame.event())
                    });
                let mut relay_send_error = None;
                let mut relay_serialization_failed = false;
                match relay_directive {
                    _ if !client_connected => {}
                    Some(ResponsesWebSocketRelayDirective::ForwardOriginal) => {
                        let client_frame = match parsed_upstream_frame.as_ref().map(|frame| {
                            bound
                                .redaction_restorer
                                .restore_provider_frame_text(frame.event())
                        }) {
                            Some(Ok(Some(text))) => Some(AxumWsMessage::Text(text.into())),
                            Some(Ok(None)) | None => {
                                Some(upstream_message_to_client(upstream_message.clone()))
                            }
                            Some(Err(_)) => {
                                relay_serialization_failed = true;
                                None
                            }
                        };
                        if let Some(client_frame) = client_frame {
                            match send_client_message(client_socket, client_frame).await {
                                Ok(()) => {
                                    if let (Some(turn), Some(frame)) = (
                                        bound.turn_state.attempt_mut(),
                                        parsed_upstream_frame.as_ref(),
                                    ) {
                                        turn.capture_client_frame(frame.event());
                                    }
                                }
                                Err(error) => relay_send_error = Some(error),
                            }
                        }
                    }
                    Some(ResponsesWebSocketRelayDirective::ForwardEvents(events)) => {
                        for event in events {
                            let text = match bound
                                .redaction_restorer
                                .restore_provider_frame_text(event)
                            {
                                Ok(Some(restored)) => restored,
                                Ok(None) => match encode_opaque_websocket_event(event) {
                                    Ok(encoded) => encoded,
                                    Err(_) => {
                                        relay_serialization_failed = true;
                                        break;
                                    }
                                },
                                Err(_) => {
                                    relay_serialization_failed = true;
                                    break;
                                }
                            };
                            match send_client_message(
                                client_socket,
                                AxumWsMessage::Text(text.into()),
                            )
                            .await
                            {
                                Ok(()) => {
                                    if let Some(turn) = bound.turn_state.attempt_mut() {
                                        turn.capture_client_frame(event);
                                    }
                                }
                                Err(error) => {
                                    relay_send_error = Some(error);
                                    break;
                                }
                            }
                        }
                    }
                    Some(ResponsesWebSocketRelayDirective::SuppressProviderPrivate) => {}
                    None => {
                        if let Err(error) = send_client_message(
                            client_socket,
                            upstream_message_to_client(upstream_message.clone()),
                        )
                        .await
                        {
                            relay_send_error = Some(error);
                        }
                    }
                }
                if relay_serialization_failed {
                    warn!(
                        event_name = "responses_websocket_provider_event_serialization_failed",
                        log_type = "ops",
                        transport = WEBSOCKET_LOG_TRANSPORT,
                        websocket = true,
                        trace_id = %context.trace_id,
                        provider_terminal_reached = terminal_outcome.is_some(),
                        "gateway could not serialize an opaque provider event"
                    );
                    bound
                        .turn_state
                        .record_client_delivery_aborted(CLIENT_DELIVERY_FAILED_REASON);
                    finalize_active_turn(
                        bound,
                        state,
                        settle_signal_for_client_delivery_failure(terminal_outcome),
                    )
                    .await;
                    send_gateway_error_with_status(
                        client_socket,
                        502,
                        "responses_websocket_event_serialization_failed",
                        "Gateway could not relay the provider event",
                    )
                    .await;
                    close_bound_upstream(bound).await;
                    close_client_socket(
                        client_socket,
                        CLOSE_INTERNAL_ERROR,
                        "provider_event_serialization_failed",
                    )
                    .await;
                    break;
                }
                if let Some(error) = relay_send_error {
                    if terminal_outcome.is_none() && retain_disconnected_turn(bound) {
                        client_connected = false;
                        continue;
                    }
                    warn!(
                        event_name = "responses_websocket_client_send_failed",
                        log_type = "ops",
                        transport = WEBSOCKET_LOG_TRANSPORT,
                        websocket = true,
                        trace_id = %context.trace_id,
                        error_code = error.as_str(),
                        provider_terminal_reached = terminal_outcome.is_some(),
                        "gateway could not relay a provider event to the client"
                    );
                    // 投递失败是独立事实，不能覆盖已经到达的 provider 终态：
                    // 供应商已经完成推理并消耗 token，账单按它的终态计。
                    bound
                        .turn_state
                        .record_client_delivery_aborted(CLIENT_DELIVERY_FAILED_REASON);
                    finalize_active_turn(
                        bound,
                        state,
                        settle_signal_for_client_delivery_failure(terminal_outcome),
                    ).await;
                    close_bound_upstream(bound).await;
                    break;
                }
                if let Some(outcome) = terminal_outcome {
                    finalize_active_turn(bound, state, outcome).await;
                } else if is_close {
                    finalize_active_turn(
                        bound,
                        state,
                        ResponsesWebSocketTurnOutcome::upstream_closed(),
                    )
                    .await;
                }
                if detach_after_forward {
                    let directive = bound
                        .pending_adapter_drain
                        .expect("adapter drain state should be present");
                    if bound.turn_state.response_in_flight() {
                        finalize_active_turn(
                            bound,
                            state,
                            ResponsesWebSocketTurnOutcome::provider_quota_exhausted(),
                        )
                        .await;
                    }
                    detach_exhausted_upstream(bound, directive, &context.trace_id).await;
                    continue;
                }
                if drain_for_adapter {
                    let directive = bound
                        .pending_adapter_drain
                        .expect("adapter drain state should be present");
                    detach_exhausted_upstream(bound, directive, &context.trace_id).await;
                    continue;
                }
                if is_close {
                    bound.upstream = None;
                    break;
                }
            }
        }
    }
}

fn retain_disconnected_turn(bound: &mut BoundResponsesConnection) -> bool {
    if bound
        .turn_state
        .attempt()
        .is_none_or(|attempt| attempt.cancel_on_client_disconnect())
    {
        return false;
    }
    bound
        .turn_state
        .record_client_delivery_aborted(CLIENT_DELIVERY_FAILED_REASON);
    true
}

struct PendingContinuationRegistration {
    user_id: String,
    api_key_id: String,
    response_id: String,
    record: ResponsesWebSocketContinuationRecord,
}

fn prepare_persisted_continuation_registration(
    bound: &BoundResponsesConnection,
    context: &WebSocketRequestContext,
    frame: &ParsedResponsesWebSocketFrame<'_>,
) -> Option<PendingContinuationRegistration> {
    let logical = bound.turn_state.logical();
    let Some(response_id) = persistable_terminal_response_id(logical, frame).map(str::to_string)
    else {
        return None;
    };
    let logical = logical.expect("a persistable terminal requires an active logical turn");
    let Some(auth_context) = logical
        .turn_control
        .as_ref()
        .and_then(|control| control.decision.auth_context.as_ref())
    else {
        warn!(
            event_name = "responses_websocket_continuation_registration_skipped",
            log_type = "ops",
            transport = WEBSOCKET_LOG_TRANSPORT,
            websocket = true,
            trace_id = %context.trace_id,
            reason = "missing_live_auth_context",
            "gateway did not register a persisted Responses continuation"
        );
        return None;
    };
    let Some(pinned_candidate) =
        crate::ai_serving::ResponsesWebSocketPinnedCandidate::from_decision(
            &bound.decision_template,
        )
    else {
        warn!(
            event_name = "responses_websocket_continuation_registration_skipped",
            log_type = "ops",
            transport = WEBSOCKET_LOG_TRANSPORT,
            websocket = true,
            trace_id = %context.trace_id,
            reason = "missing_binding_identity",
            "gateway did not register a persisted Responses continuation"
        );
        return None;
    };
    let record = match ResponsesWebSocketContinuationRecord::from_binding(
        pinned_candidate,
        bound.client_model.as_str(),
        bound.provider_model.as_str(),
        &bound.binding_identity,
        &bound.body_normalization,
        bound.redaction_restorer.has_sessions(),
        bound.responses_lite_static_config.clone(),
    ) {
        Ok(record) => record,
        Err(error) => {
            warn!(
                event_name = "responses_websocket_continuation_registration_skipped",
                log_type = "ops",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                provider_id = %bound.decision_template.provider_id.as_deref().unwrap_or("-"),
                endpoint_id = %bound.decision_template.endpoint_id.as_deref().unwrap_or("-"),
                key_id = %bound.decision_template.key_id.as_deref().unwrap_or("-"),
                reason = error.kind(),
                "gateway could not build a persisted Responses continuation record"
            );
            return None;
        }
    };
    Some(PendingContinuationRegistration {
        user_id: auth_context.user_id.clone(),
        api_key_id: auth_context.api_key_id.clone(),
        response_id,
        record,
    })
}

fn persistable_terminal_response_id<'a>(
    logical: Option<&LogicalTurn>,
    frame: &'a ParsedResponsesWebSocketFrame<'_>,
) -> Option<&'a str> {
    let logical = logical?;
    // `provider_store` is derived from the final framed provider event. False
    // or absent is ZDR/connection-local and must never create a 24-hour KV
    // record, even if the provider happens to return an ID.
    if !logical.provider_store {
        return None;
    }
    connection_local_terminal_response_id(Some(logical), frame)
}

fn connection_local_terminal_response_id<'a>(
    logical: Option<&LogicalTurn>,
    frame: &'a ParsedResponsesWebSocketFrame<'_>,
) -> Option<&'a str> {
    logical?;
    frame.continuation_response_id()
}

fn evicted_default_lane_continuation_response_id<'a>(
    logical: Option<&'a LogicalTurn>,
    frame: &ParsedResponsesWebSocketFrame<'_>,
) -> Option<&'a str> {
    let logical = logical?;
    let terminal = frame.terminal()?;
    if terminal.status_code < 400 || terminal.cancelled {
        return None;
    }
    logical
        .client_event
        .get("previous_response_id")
        .and_then(Value::as_str)
        .filter(|response_id| !response_id.trim().is_empty())
}

async fn register_persisted_continuation_before_terminal_delivery(
    state: &AppState,
    context: &WebSocketRequestContext,
    registration: PendingContinuationRegistration,
) -> Option<String> {
    let PendingContinuationRegistration {
        user_id,
        api_key_id,
        response_id,
        record,
    } = registration;
    let registry = ResponsesWebSocketContinuationRegistry::new(state.runtime_state.as_ref());
    match tokio::time::timeout(
        CONTINUATION_REGISTRATION_TIMEOUT,
        registry.register(
            user_id.as_str(),
            api_key_id.as_str(),
            response_id.as_str(),
            &record,
        ),
    )
    .await
    {
        Ok(Ok(())) => {
            debug!(
                event_name = "responses_websocket_continuation_registered",
                log_type = "event",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                user_id = %user_id,
                api_key_id = %api_key_id,
                provider_id = %record.pinned_candidate().provider_id(),
                endpoint_id = %record.pinned_candidate().endpoint_id(),
                key_id = %record.pinned_candidate().key_id(),
                client_model = %record.client_model(),
                provider_model = %record.provider_model(),
                "gateway registered a persisted Responses continuation before terminal delivery"
            );
            Some(response_id)
        }
        Ok(Err(error)) => {
            warn!(
                event_name = "responses_websocket_continuation_registration_failed",
                log_type = "ops",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                provider_id = %record.pinned_candidate().provider_id(),
                endpoint_id = %record.pinned_candidate().endpoint_id(),
                key_id = %record.pinned_candidate().key_id(),
                reason = error.kind(),
                "gateway failed to register a persisted Responses continuation"
            );
            None
        }
        Err(_) => {
            warn!(
                event_name = "responses_websocket_continuation_registration_failed",
                log_type = "ops",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                provider_id = %record.pinned_candidate().provider_id(),
                endpoint_id = %record.pinned_candidate().endpoint_id(),
                key_id = %record.pinned_candidate().key_id(),
                reason = "timeout",
                timeout_ms = CONTINUATION_REGISTRATION_TIMEOUT.as_millis() as u64,
                "gateway timed out registering a persisted Responses continuation"
            );
            None
        }
    }
}

pub(super) async fn wait_for_connection_permit_loss(
    permit: Option<&aether_runtime::AdmissionPermit>,
) {
    let Some(permit) = permit else {
        std::future::pending::<()>().await;
        return;
    };
    let mut health = tokio::time::interval(Duration::from_secs(1));
    health.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        health.tick().await;
        if !permit.is_healthy() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        connection_local_terminal_response_id, evicted_default_lane_continuation_response_id,
        persistable_terminal_response_id, LogicalTurn, ParsedResponsesWebSocketFrame,
    };

    #[test]
    fn cross_connection_registration_requires_explicit_store_true_and_a_success_terminal() {
        let completed = ParsedResponsesWebSocketFrame::parse(
            r#"{"type":"response.completed","response":{"id":"resp_persisted"}}"#,
        )
        .expect("valid completed event");
        let failed = ParsedResponsesWebSocketFrame::parse(
            r#"{"type":"response.failed","response":{"id":"resp_failed"}}"#,
        )
        .expect("valid failed event");

        let omitted_or_false = LogicalTurn::new(
            json!({"type": "response.create", "store": false}),
            1,
            "logical-local".to_string(),
        );
        assert_eq!(
            persistable_terminal_response_id(Some(&omitted_or_false), &completed),
            None,
            "store=false or an omitted provider-side store must remain connection-local"
        );
        assert_eq!(persistable_terminal_response_id(None, &completed), None);
        assert_eq!(
            connection_local_terminal_response_id(Some(&omitted_or_false), &completed),
            Some("resp_persisted"),
            "store=false continuations remain valid on the same physical socket"
        );
        assert_eq!(
            connection_local_terminal_response_id(None, &completed),
            None
        );

        let persisted = LogicalTurn::new(
            json!({"type": "response.create", "store": true}),
            1,
            "logical-persisted".to_string(),
        )
        .with_provider_store(true);
        assert_eq!(
            persistable_terminal_response_id(Some(&persisted), &completed),
            Some("resp_persisted")
        );
        assert_eq!(
            persistable_terminal_response_id(Some(&persisted), &failed),
            None,
            "a failed terminal must never establish cross-connection ownership"
        );

        let continuation = LogicalTurn::new(
            json!({
                "type": "response.create",
                "previous_response_id": "resp_parent"
            }),
            2,
            "logical-continuation".to_string(),
        );
        assert_eq!(
            evicted_default_lane_continuation_response_id(Some(&continuation), &failed),
            Some("resp_parent")
        );
        assert_eq!(
            evicted_default_lane_continuation_response_id(Some(&continuation), &completed),
            None
        );
    }
}
