//! Opaque bidirectional relay for the public OpenAI Realtime WebSocket API.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::ws::{Message as AxumWsMessage, WebSocket};
use axum::http::StatusCode;
use futures_util::{SinkExt, StreamExt};
use tracing::{info, warn};
use wreq::ws::message::Message as WreqWsMessage;

use crate::control::execution_plan_balance_capacity_rejection;
use crate::handlers::proxy::websocket::ingress::{
    WebSocketConnectionLog, WebSocketConnectionLogSpec, WebSocketRequestContext,
};
use crate::handlers::proxy::websocket::responses::ResponsesWebSocketTurnAdmission;
use crate::handlers::proxy::websocket::session::{
    CLOSE_INTERNAL_ERROR, CLOSE_POLICY_VIOLATION, CLOSE_TRY_AGAIN,
    REALTIME_WEBSOCKET_SESSION_LIMITS, WEBSOCKET_LOG_TRANSPORT,
};
use crate::handlers::proxy::websocket::transport::{
    client_message_to_upstream, close_client_socket, close_upstream_socket,
    connect_upstream_websocket, send_client_message, upstream_message_to_client,
    websocket_relay_frame_queue, UpstreamWebSocketErrorCodes, WebSocketRelayPumpControl,
    WebSocketRelayQueueError, WebSocketWriteError,
};
use crate::{AppState, GatewayError};

use super::audit::{RealtimeSessionAudit, RealtimeSessionDisposition, RealtimeSessionTerminal};
use super::planner::{plan_realtime_candidate, PlannedRealtimeCandidate};
use super::protocol::{error_event, model_from_query, RealtimeUsageObserver};

const REALTIME_LOG_TARGET: &str = "aether_gateway::handlers::proxy::realtime_ws";
const REALTIME_CONNECTION_LOG_SPEC: WebSocketConnectionLogSpec = WebSocketConnectionLogSpec {
    opened_event_name: "openai_realtime_websocket_connection_opened",
    closed_event_name: "openai_realtime_websocket_connection_closed",
    opened_message: "gateway accepted OpenAI Realtime WebSocket connection",
    closed_message: "gateway closed OpenAI Realtime WebSocket connection",
    execution_path: "openai_realtime_websocket_bridge",
    provider_type: "openai_realtime",
};
const REALTIME_UPSTREAM_ERRORS: UpstreamWebSocketErrorCodes = UpstreamWebSocketErrorCodes {
    upstream_url_missing: "openai_realtime_upstream_url_missing",
    upstream_url_invalid: "openai_realtime_upstream_url_invalid",
    frontdoor_self_loop: "openai_realtime_websocket_frontdoor_self_loop",
    headers_invalid: "openai_realtime_websocket_headers_invalid",
    client_build_failed: "openai_realtime_websocket_client_build_failed",
    proxy_invalid: "openai_realtime_websocket_proxy_invalid",
    tunnel_proxy_unsupported: "openai_realtime_websocket_tunnel_proxy_unsupported",
    handshake_failed: "openai_realtime_websocket_handshake_failed",
    upgrade_rejected: "openai_realtime_websocket_upgrade_rejected",
    upgrade_failed: "openai_realtime_websocket_upgrade_failed",
};

pub(super) struct PreparedRealtimeWebSocket {
    upstream: wreq::ws::WebSocket,
    admission: ResponsesWebSocketTurnAdmission,
    candidate: PlannedRealtimeCandidate,
}

pub(super) struct RealtimeWebSocketPreflightRejection {
    status: StatusCode,
    message: String,
}

impl RealtimeWebSocketPreflightRejection {
    pub(super) const fn status(&self) -> StatusCode {
        self.status
    }

    pub(super) fn message(&self) -> &str {
        self.message.as_str()
    }
}

pub(super) async fn prepare_realtime_websocket(
    state: &AppState,
    context: &WebSocketRequestContext,
) -> Result<PreparedRealtimeWebSocket, RealtimeWebSocketPreflightRejection> {
    if !realtime_usage_accounting_is_safe(context) {
        return Err(rejection(
            StatusCode::NOT_IMPLEMENTED,
            "Realtime WebSocket is unavailable for finite-balance keys until session usage settlement is enabled",
        ));
    }
    let client_model = model_from_query(context.uri.query())
        .map_err(|error| rejection(StatusCode::BAD_REQUEST, error.client_message()))?;
    let candidate = plan_realtime_candidate(state, context, client_model.as_str())
        .await
        .map_err(|error| {
            warn!(
                target: REALTIME_LOG_TARGET,
                event_name = "openai_realtime_planning_failed",
                log_type = "ops",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                error_kind = gateway_error_kind(&error),
                "OpenAI Realtime candidate planning failed"
            );
            rejection(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Realtime provider planning failed",
            )
        })?
        .ok_or_else(|| {
            rejection(
                StatusCode::SERVICE_UNAVAILABLE,
                "No eligible OpenAI Realtime provider mapping is available",
            )
        })?;

    if execution_plan_balance_capacity_rejection(
        state,
        &context.decision,
        &candidate.admission_plan,
        candidate.execution.report_context.as_ref(),
    )
    .await
    .map_err(|_| {
        rejection(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Realtime balance admission failed",
        )
    })?
    .is_some()
    {
        candidate.pool_lease.release().await;
        return Err(rejection(
            StatusCode::TOO_MANY_REQUESTS,
            "Realtime request capacity is unavailable",
        ));
    }

    let admission = match ResponsesWebSocketTurnAdmission::acquire(
        state,
        &candidate.admission_plan,
        context.trace_id.as_str(),
    )
    .await
    {
        Ok(admission) => admission,
        Err(error) => {
            candidate.pool_lease.release().await;
            return Err(rejection(
                admission_error_status(&error),
                "Realtime connection admission failed",
            ));
        }
    };
    if !candidate.pool_lease.is_healthy() {
        admission.release().await;
        candidate.pool_lease.release().await;
        return Err(rejection(
            StatusCode::SERVICE_UNAVAILABLE,
            "Realtime provider ownership was lost",
        ));
    }
    let mut upstream = match connect_upstream_websocket(
        &candidate.execution,
        REALTIME_WEBSOCKET_SESSION_LIMITS,
        REALTIME_UPSTREAM_ERRORS,
    )
    .await
    {
        Ok(connection) => connection.socket,
        Err(error_code) => {
            warn!(
                target: REALTIME_LOG_TARGET,
                event_name = "openai_realtime_upstream_connect_failed",
                log_type = "ops",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                provider_id = %candidate.provider_id,
                endpoint_id = %candidate.endpoint_id,
                key_id = %candidate.key_id,
                error_code,
                "OpenAI Realtime upstream connection failed"
            );
            admission.release().await;
            candidate.pool_lease.release().await;
            return Err(rejection(
                StatusCode::BAD_GATEWAY,
                "Realtime upstream WebSocket connection failed",
            ));
        }
    };
    // The scheduler lease can expire while the upstream WebSocket handshake
    // is in flight. Re-check it after the handshake so an invalid provider
    // candidate is rejected before the downstream HTTP 101 is committed.
    if !candidate.pool_lease.is_healthy() {
        close_upstream_socket(&mut upstream, None).await;
        admission.release().await;
        candidate.pool_lease.release().await;
        return Err(rejection(
            StatusCode::SERVICE_UNAVAILABLE,
            "Realtime provider ownership was lost",
        ));
    }

    Ok(PreparedRealtimeWebSocket {
        upstream,
        admission,
        candidate,
    })
}

pub(super) async fn run_realtime_websocket(
    mut client_socket: WebSocket,
    state: AppState,
    context: WebSocketRequestContext,
    prepared: PreparedRealtimeWebSocket,
) {
    let connection_log = WebSocketConnectionLog::new(&context, REALTIME_CONNECTION_LOG_SPEC);
    connection_log.log_opened();
    let PreparedRealtimeWebSocket {
        mut upstream,
        admission,
        candidate,
    } = prepared;
    let audit = RealtimeSessionAudit::new(
        &candidate.admission_plan,
        candidate.execution.report_context.as_ref(),
    );
    let terminal = relay_realtime(&mut client_socket, &mut upstream, &context, &candidate).await;
    close_upstream_socket(&mut upstream, None).await;
    admission.release().await;
    candidate.pool_lease.release().await;
    if matches!(
        terminal.termination,
        "connection_duration_limit" | "connection_admission_lost" | "pool_key_lease_lost"
    ) {
        close_client_socket(&mut client_socket, CLOSE_TRY_AGAIN, terminal.termination).await;
    }
    audit.finish(&state, terminal).await;
}

async fn relay_realtime(
    client_socket: &mut WebSocket,
    upstream: &mut wreq::ws::WebSocket,
    context: &WebSocketRequestContext,
    candidate: &PlannedRealtimeCandidate,
) -> RealtimeSessionTerminal {
    let started_at = Instant::now();
    let connection_deadline =
        tokio::time::sleep(REALTIME_WEBSOCKET_SESSION_LIMITS.max_connection_duration);
    tokio::pin!(connection_deadline);
    let mut lease_health = tokio::time::interval(Duration::from_secs(1));
    lease_health.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let stats = Arc::new(Mutex::new(RelayStats::default()));
    let usage = Arc::new(Mutex::new(RealtimeUsageObserver::default()));
    let relay_control = WebSocketRelayPumpControl::new();

    let termination = {
        let (mut client_write, mut client_read) = (&mut *client_socket).split();
        let (mut upstream_write, mut upstream_read) = (&mut *upstream).split();

        let client_to_upstream = {
            let control = relay_control.clone();
            let stats = Arc::clone(&stats);
            async move {
                let (queue_tx, mut queue_rx) = websocket_relay_frame_queue();
                let reader_control = control.clone();
                let reader = async move {
                    loop {
                        let client = tokio::select! {
                            biased;
                            _ = reader_control.cancelled() => return "relay_cancelled",
                            client = client_read.next() => client,
                        };
                        let Some(client) = client else {
                            return "client_closed";
                        };
                        let Ok(client) = client else {
                            return "client_read_failed";
                        };
                        let (bytes, is_close) = client_frame_metadata(&client);
                        {
                            let mut stats = stats
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            stats.client_frames = stats.client_frames.saturating_add(1);
                            stats.client_bytes = stats.client_bytes.saturating_add(bytes as u64);
                        }
                        match reader_control
                            .enqueue(&queue_tx, client_message_to_upstream(client))
                            .await
                        {
                            Ok(()) => {}
                            Err(WebSocketRelayQueueError::Cancelled) => {
                                return "relay_cancelled";
                            }
                            Err(WebSocketRelayQueueError::Closed) => {
                                return "upstream_write_failed";
                            }
                        }
                        if is_close {
                            return "client_close_frame";
                        }
                    }
                };

                let writer_control = control;
                let writer = async move {
                    loop {
                        let message = tokio::select! {
                            biased;
                            _ = writer_control.cancelled() => return None,
                            message = queue_rx.recv() => message,
                        };
                        let Some(message) = message else {
                            return None;
                        };
                        let result = writer_control
                            .send(async { upstream_write.send(message).await.map_err(|_| ()) })
                            .await;
                        match result {
                            Ok(()) => {}
                            Err(WebSocketWriteError::Cancelled) => return None,
                            Err(_) => return Some("upstream_write_failed"),
                        }
                    }
                };

                tokio::pin!(reader, writer);
                tokio::select! {
                    reader_exit = &mut reader => {
                        writer.await.unwrap_or(reader_exit)
                    }
                    writer_exit = &mut writer => {
                        match writer_exit {
                            Some(writer_exit) => writer_exit,
                            None => reader.await,
                        }
                    }
                }
            }
        };

        let upstream_to_client = {
            let control = relay_control.clone();
            let stats = Arc::clone(&stats);
            let usage = Arc::clone(&usage);
            async move {
                let (queue_tx, mut queue_rx) = websocket_relay_frame_queue();
                let reader_control = control.clone();
                let reader = async move {
                    loop {
                        let provider = tokio::select! {
                            biased;
                            _ = reader_control.cancelled() => return "relay_cancelled",
                            provider = upstream_read.next() => provider,
                        };
                        let Some(provider) = provider else {
                            return "upstream_closed";
                        };
                        let Ok(provider) = provider else {
                            return "upstream_read_failed";
                        };
                        let (bytes, is_close) = upstream_frame_metadata(&provider);
                        {
                            let mut stats = stats
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            stats.first_upstream_frame_ms.get_or_insert_with(|| {
                                started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
                            });
                            stats.upstream_frames = stats.upstream_frames.saturating_add(1);
                            stats.upstream_bytes =
                                stats.upstream_bytes.saturating_add(bytes as u64);
                        }
                        if let WreqWsMessage::Text(text) = &provider {
                            usage
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .observe(text.as_str());
                        }
                        match reader_control
                            .enqueue(&queue_tx, upstream_message_to_client(provider))
                            .await
                        {
                            Ok(()) => {}
                            Err(WebSocketRelayQueueError::Cancelled) => {
                                return "relay_cancelled";
                            }
                            Err(WebSocketRelayQueueError::Closed) => {
                                return "client_write_failed";
                            }
                        }
                        if is_close {
                            return "upstream_close_frame";
                        }
                    }
                };

                let writer_control = control;
                let writer = async move {
                    loop {
                        let message = tokio::select! {
                            biased;
                            _ = writer_control.cancelled() => return None,
                            message = queue_rx.recv() => message,
                        };
                        let Some(message) = message else {
                            return None;
                        };
                        let result = writer_control
                            .send(async { client_write.send(message).await.map_err(|_| ()) })
                            .await;
                        match result {
                            Ok(()) => {}
                            Err(WebSocketWriteError::Cancelled) => return None,
                            Err(_) => return Some("client_write_failed"),
                        }
                    }
                };

                tokio::pin!(reader, writer);
                tokio::select! {
                    reader_exit = &mut reader => {
                        writer.await.unwrap_or(reader_exit)
                    }
                    writer_exit = &mut writer => {
                        match writer_exit {
                            Some(writer_exit) => writer_exit,
                            None => reader.await,
                        }
                    }
                }
            }
        };

        tokio::pin!(client_to_upstream, upstream_to_client);
        let termination = loop {
            tokio::select! {
                termination = &mut client_to_upstream => break termination,
                termination = &mut upstream_to_client => break termination,
                _ = &mut connection_deadline => break "connection_duration_limit",
                _ = wait_for_connection_permit_loss(context.websocket_connection_permit.as_ref()) => {
                    break "connection_admission_lost";
                }
                _ = lease_health.tick() => {
                    if !candidate.pool_lease.is_healthy() {
                        break "pool_key_lease_lost";
                    }
                }
            }
        };
        relay_control.cancel();
        termination
    };
    if termination == "pool_key_lease_lost" {
        send_realtime_error(
            client_socket,
            "openai_realtime_pool_key_lease_lost",
            "Realtime provider ownership was lost",
        )
        .await;
    }
    let stats = *stats
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let totals = usage
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .totals();
    let elapsed_ms = started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    info!(
        target: REALTIME_LOG_TARGET,
        event_name = "openai_realtime_relay_finished",
        log_type = "event",
        transport = WEBSOCKET_LOG_TRANSPORT,
        websocket = true,
        trace_id = %context.trace_id,
        provider_id = %candidate.provider_id,
        endpoint_id = %candidate.endpoint_id,
        key_id = %candidate.key_id,
        model = %candidate.provider_model,
        termination,
        client_frames = stats.client_frames,
        client_bytes = stats.client_bytes,
        upstream_frames = stats.upstream_frames,
        upstream_bytes = stats.upstream_bytes,
        response_count = totals.responses,
        input_tokens = totals.input_tokens,
        output_tokens = totals.output_tokens,
        total_tokens = totals.total_tokens,
        cached_input_tokens = totals.cached_input_tokens,
        input_audio_tokens = totals.input_audio_tokens,
        output_audio_tokens = totals.output_audio_tokens,
        elapsed_ms,
        "OpenAI Realtime opaque relay finished"
    );
    realtime_terminal_from_relay(termination, elapsed_ms, stats, totals)
}

fn realtime_terminal_from_relay(
    termination: &'static str,
    elapsed_ms: u64,
    stats: RelayStats,
    usage: super::protocol::RealtimeUsageTotals,
) -> RealtimeSessionTerminal {
    let (disposition, status_code) = match termination {
        "client_close_frame" | "upstream_close_frame" => {
            (RealtimeSessionDisposition::Completed, 200)
        }
        "client_closed"
        | "client_read_failed"
        | "client_write_failed"
        | "connection_duration_limit" => (RealtimeSessionDisposition::Cancelled, 499),
        "pool_key_lease_lost" | "connection_admission_lost" => {
            (RealtimeSessionDisposition::Failed, 503)
        }
        "upstream_closed" | "upstream_read_failed" | "upstream_write_failed" => {
            (RealtimeSessionDisposition::Failed, 502)
        }
        _ => (RealtimeSessionDisposition::Failed, 500),
    };
    RealtimeSessionTerminal {
        disposition,
        status_code,
        termination,
        elapsed_ms,
        first_upstream_frame_ms: stats.first_upstream_frame_ms,
        client_frames: stats.client_frames,
        client_bytes: stats.client_bytes,
        upstream_frames: stats.upstream_frames,
        upstream_bytes: stats.upstream_bytes,
        usage,
    }
}

fn realtime_usage_accounting_is_safe(context: &WebSocketRequestContext) -> bool {
    context
        .decision
        .auth_context
        .as_ref()
        .is_some_and(|auth| auth.balance_remaining.is_none())
}

fn rejection(
    status: StatusCode,
    message: impl Into<String>,
) -> RealtimeWebSocketPreflightRejection {
    RealtimeWebSocketPreflightRejection {
        status,
        message: message.into(),
    }
}

fn admission_error_status(error: &GatewayError) -> StatusCode {
    match error {
        GatewayError::AdmissionTimeout { .. } => StatusCode::TOO_MANY_REQUESTS,
        GatewayError::PlanUsageLimited(_) => StatusCode::TOO_MANY_REQUESTS,
        GatewayError::LastActiveAdminUpdateDenied | GatewayError::LastActiveAdminDeleteDenied => {
            StatusCode::BAD_REQUEST
        }
        GatewayError::Client { status, .. } => *status,
        GatewayError::LocalExecutionPlanningTimeout { .. } => StatusCode::GATEWAY_TIMEOUT,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn gateway_error_kind(error: &GatewayError) -> &'static str {
    match error {
        GatewayError::UpstreamUnavailable { .. } => "upstream_unavailable",
        GatewayError::ControlUnavailable { .. } => "control_unavailable",
        GatewayError::LocalExecutionPlanningTimeout { .. } => "planning_timeout",
        GatewayError::AdmissionTimeout { .. } => "admission_timeout",
        GatewayError::PlanUsageLimited(_) => "plan_usage_limited",
        GatewayError::LastActiveAdminUpdateDenied => "last_admin_update_denied",
        GatewayError::LastActiveAdminDeleteDenied => "last_admin_delete_denied",
        GatewayError::Client { .. } => "client_error",
        GatewayError::Internal(_) => "internal_error",
    }
}

async fn send_realtime_error(client_socket: &mut WebSocket, code: &str, message: &str) {
    let event = error_event(code, message).to_string();
    let _ = send_client_message(client_socket, AxumWsMessage::Text(event.into())).await;
}

#[derive(Clone, Copy, Default)]
struct RelayStats {
    client_frames: u64,
    client_bytes: u64,
    upstream_frames: u64,
    upstream_bytes: u64,
    first_upstream_frame_ms: Option<u64>,
}

fn client_frame_metadata(message: &AxumWsMessage) -> (usize, bool) {
    match message {
        AxumWsMessage::Text(text) => (text.len(), false),
        AxumWsMessage::Binary(data) | AxumWsMessage::Ping(data) | AxumWsMessage::Pong(data) => {
            (data.len(), false)
        }
        AxumWsMessage::Close(frame) => (
            frame
                .as_ref()
                .map_or(0, |frame| 2usize.saturating_add(frame.reason.len())),
            true,
        ),
    }
}

fn upstream_frame_metadata(message: &WreqWsMessage) -> (usize, bool) {
    match message {
        WreqWsMessage::Text(text) => (text.len(), false),
        WreqWsMessage::Binary(data) | WreqWsMessage::Ping(data) | WreqWsMessage::Pong(data) => {
            (data.len(), false)
        }
        WreqWsMessage::Close(frame) => (
            frame
                .as_ref()
                .map_or(0, |frame| 2usize.saturating_add(frame.reason.len())),
            true,
        ),
    }
}

async fn wait_for_connection_permit_loss(permit: Option<&aether_runtime::AdmissionPermit>) {
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
    use super::{client_frame_metadata, upstream_frame_metadata};
    use axum::extract::ws::Message as AxumWsMessage;
    use wreq::ws::message::Message as WreqWsMessage;

    #[test]
    fn opaque_frame_accounting_does_not_coalesce_audio_or_json_messages() {
        assert_eq!(
            client_frame_metadata(&AxumWsMessage::Text("{\"type\":\"session.update\"}".into())),
            (25, false)
        );
        assert_eq!(
            client_frame_metadata(&AxumWsMessage::Binary(vec![1, 2, 3].into())),
            (3, false)
        );
        assert_eq!(
            upstream_frame_metadata(&WreqWsMessage::Text("delta".into())),
            (5, false)
        );
    }
}
