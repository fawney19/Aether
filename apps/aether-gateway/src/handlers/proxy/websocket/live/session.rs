//! Opaque direct and WebRTC-sideband WebSocket relay for Codex Live.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::ws::{Message as AxumWsMessage, WebSocket};
use axum::http::StatusCode;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tracing::{info, warn};
use wreq::ws::message::Message as WreqWsMessage;

use crate::control::execution_plan_balance_capacity_rejection;
use crate::handlers::proxy::websocket::ingress::{
    WebSocketConnectionLog, WebSocketConnectionLogSpec, WebSocketRequestContext,
};
use crate::handlers::proxy::websocket::responses::ResponsesWebSocketTurnAdmission;
use crate::handlers::proxy::websocket::session::{
    wait_for_optional_deadline, CLOSE_POLICY_VIOLATION, CLOSE_TRY_AGAIN,
    LIVE_WEBSOCKET_SESSION_LIMITS, WEBSOCKET_LOG_TRANSPORT,
};
use crate::handlers::proxy::websocket::transport::{
    client_message_to_upstream, close_client_socket, close_upstream_socket,
    connect_upstream_websocket, send_client_message, send_upstream_message,
    upstream_message_to_client, websocket_relay_frame_queue, UpstreamWebSocketErrorCodes,
    WebSocketRelayPumpControl, WebSocketRelayQueueError, WebSocketWriteError,
};
use crate::{AppState, GatewayError, LocalExecutionRuntimeMissDiagnostic};

use super::audit::{
    LiveAuditTransport, LiveSessionAudit, LiveSessionDisposition, LiveSessionTerminal,
};
use super::live_usage_accounting_is_safe;
use super::planner::{
    build_live_stream_admission_attempt, direct_live_websocket_url_for_dialect, live_sideband_url,
    plan_live_candidate, LiveCandidatePlanningOutcome, LivePoolLeaseGuard, PlannedLiveCandidate,
};
use super::protocol::{
    direct_model_from_query, direct_realtime_model_from_query, direct_realtime_v2_model_from_query,
    event_type, realtime_sideband_query_has_call_id, realtime_v2_request_is_codex,
    sideband_call_from_request, validate_initial_session_update, LiveRouteDialect,
    LEGACY_LIVE_CALL_PATH, REALTIME_SIDEBAND_PATH,
};
use super::registry::{
    LiveCallLookup, LiveCallRecord, LiveCallRegistry, LiveCallRegistryError, LiveSidebandLease,
    LiveSidebandLeaseLoss,
};

const LIVE_LOG_TARGET: &str = "aether_gateway::handlers::proxy::codex_live";
const SIDEBAND_LOOKUP_TIMEOUT: Duration = Duration::from_millis(500);
const SESSION_CLOSE_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
const LIVE_CONNECTION_LOG_SPEC: WebSocketConnectionLogSpec = WebSocketConnectionLogSpec {
    opened_event_name: "codex_live_websocket_connection_opened",
    closed_event_name: "codex_live_websocket_connection_closed",
    opened_message: "gateway accepted Codex Live WebSocket connection",
    closed_message: "gateway closed Codex Live WebSocket connection",
    execution_path: "codex_live_websocket_bridge",
    provider_type: "codex_live",
};
const LIVE_UPSTREAM_ERRORS: UpstreamWebSocketErrorCodes = UpstreamWebSocketErrorCodes {
    upstream_url_missing: "codex_live_upstream_url_missing",
    upstream_url_invalid: "codex_live_upstream_url_invalid",
    frontdoor_self_loop: "codex_live_websocket_frontdoor_self_loop",
    headers_invalid: "codex_live_websocket_headers_invalid",
    client_build_failed: "codex_live_websocket_client_build_failed",
    proxy_invalid: "codex_live_websocket_proxy_invalid",
    tunnel_proxy_unsupported: "codex_live_websocket_tunnel_proxy_unsupported",
    handshake_failed: "codex_live_websocket_handshake_failed",
    upgrade_rejected: "codex_live_websocket_upgrade_rejected",
    upgrade_failed: "codex_live_websocket_upgrade_failed",
};

#[derive(Debug)]
enum LiveRelayAdmissionError {
    PlanUnavailable,
    BalanceRejected,
    Gateway(GatewayError),
}

struct LiveRelayAdmission {
    capacity: ResponsesWebSocketTurnAdmission,
    audit: LiveSessionAudit,
}

struct LiveRelayAdmissionFailure {
    error: LiveRelayAdmissionError,
    audit: Option<LiveSessionAudit>,
}

impl LiveRelayAdmissionError {
    fn status(&self) -> StatusCode {
        match self {
            Self::PlanUnavailable => StatusCode::BAD_GATEWAY,
            Self::BalanceRejected | Self::Gateway(GatewayError::AdmissionTimeout { .. }) => {
                StatusCode::TOO_MANY_REQUESTS
            }
            Self::Gateway(GatewayError::PlanUsageLimited(_)) => StatusCode::TOO_MANY_REQUESTS,
            Self::Gateway(GatewayError::LastActiveAdminUpdateDenied)
            | Self::Gateway(GatewayError::LastActiveAdminDeleteDenied) => StatusCode::BAD_REQUEST,
            Self::Gateway(GatewayError::Client { status, .. }) => *status,
            Self::Gateway(GatewayError::LocalExecutionPlanningTimeout { .. }) => {
                StatusCode::GATEWAY_TIMEOUT
            }
            Self::Gateway(GatewayError::UpstreamUnavailable { .. })
            | Self::Gateway(GatewayError::ControlUnavailable { .. }) => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            Self::Gateway(GatewayError::Internal(_)) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn client_message(&self) -> &'static str {
        match self {
            Self::PlanUnavailable => "Codex Live provider admission could not be prepared",
            Self::BalanceRejected => "Codex Live request capacity is unavailable",
            Self::Gateway(GatewayError::AdmissionTimeout { .. }) => {
                "Gateway capacity is busy; retry this Live connection"
            }
            Self::Gateway(GatewayError::PlanUsageLimited(_)) => {
                "Subscription plan usage limit reached"
            }
            Self::Gateway(GatewayError::LastActiveAdminUpdateDenied)
            | Self::Gateway(GatewayError::LastActiveAdminDeleteDenied) => {
                "Codex Live request was not allowed"
            }
            Self::Gateway(GatewayError::Client { .. }) => "Codex Live request was not allowed",
            Self::Gateway(GatewayError::LocalExecutionPlanningTimeout { .. }) => {
                "Codex Live admission planning timed out"
            }
            Self::Gateway(_) => "Gateway could not admit this Codex Live connection",
        }
    }

    fn termination(&self) -> &'static str {
        match self {
            Self::PlanUnavailable => "admission_plan_unavailable",
            Self::BalanceRejected => "balance_rejected",
            Self::Gateway(GatewayError::AdmissionTimeout { .. }) => "admission_timeout",
            Self::Gateway(GatewayError::PlanUsageLimited(_)) => "plan_usage_limited",
            Self::Gateway(GatewayError::LastActiveAdminUpdateDenied) => "last_admin_update_denied",
            Self::Gateway(GatewayError::LastActiveAdminDeleteDenied) => "last_admin_delete_denied",
            Self::Gateway(GatewayError::Client { .. }) => "request_rejected",
            Self::Gateway(GatewayError::LocalExecutionPlanningTimeout { .. }) => {
                "admission_planning_timeout"
            }
            Self::Gateway(GatewayError::UpstreamUnavailable { .. }) => "upstream_unavailable",
            Self::Gateway(GatewayError::ControlUnavailable { .. }) => "control_unavailable",
            Self::Gateway(GatewayError::Internal(_)) => "admission_failed",
        }
    }
}

pub(super) enum PreparedLiveWebSocket {
    Direct(PreparedLiveRelay),
    Sideband(PreparedLiveSideband),
}

pub(super) struct PreparedLiveRelay {
    upstream: wreq::ws::WebSocket,
    admission: ResponsesWebSocketTurnAdmission,
    audit: LiveSessionAudit,
    pool_lease: LivePoolLeaseGuard,
    provider_id: String,
    endpoint_id: String,
    key_id: String,
    provider_model: String,
}

pub(super) struct PreparedLiveSideband {
    relay: PreparedLiveRelay,
    sideband_lease: LiveSidebandLease,
}

pub(super) struct LiveWebSocketPreflightRejection {
    status: StatusCode,
    termination: &'static str,
    message: &'static str,
    audit_persisted: bool,
}

impl LiveWebSocketPreflightRejection {
    pub(super) const fn status(&self) -> StatusCode {
        self.status
    }

    pub(super) const fn message(&self) -> &'static str {
        self.message
    }

    pub(super) const fn termination(&self) -> &'static str {
        self.termination
    }

    pub(super) const fn audit_persisted(&self) -> bool {
        self.audit_persisted
    }
}

pub(super) async fn prepare_live_websocket(
    state: &AppState,
    context: &WebSocketRequestContext,
) -> Result<PreparedLiveWebSocket, LiveWebSocketPreflightRejection> {
    if !live_usage_accounting_is_safe(&context.decision) {
        warn!(
            target: LIVE_LOG_TARGET,
            event_name = "codex_live_usage_accounting_unsafe",
            log_type = "ops",
            transport = WEBSOCKET_LOG_TRANSPORT,
            websocket = true,
            trace_id = %context.trace_id,
            "Codex Live rejected a finite-balance principal before WebSocket upgrade because Frameless usage is unavailable"
        );
        return Err(preflight_rejection(
            context,
            "unknown",
            StatusCode::NOT_IMPLEMENTED,
            "usage_settlement_unavailable",
            "Codex Live is unavailable for finite-balance keys until Frameless usage settlement is supported",
        ));
    }
    if context.uri.path() == LEGACY_LIVE_CALL_PATH {
        let client_model = match direct_model_from_query(context.uri.query()) {
            Ok(model) => model,
            Err(error) => {
                return Err(preflight_rejection(
                    context,
                    "direct",
                    error.status_code(),
                    error.code(),
                    error.client_message(),
                ));
            }
        };
        return prepare_direct_live_websocket(
            state,
            context,
            client_model.as_str(),
            LiveRouteDialect::LegacyLive,
        )
        .await
        .map(PreparedLiveWebSocket::Direct);
    }
    // Codex Realtime V1 uses the OpenAI `/v1/realtime` path with
    // `intent=quicksilver&model=...`; the current default V2 omits `intent`
    // and is identified by the first-party Codex originator header.  A
    // call_id on that path is a WebRTC sideband attachment and must continue
    // through the registry branch below.
    if context.uri.path() == REALTIME_SIDEBAND_PATH
        && !realtime_sideband_query_has_call_id(context.uri.query())
    {
        let (client_model, dialect) = match direct_realtime_model_from_query(context.uri.query()) {
            Ok(model) => (model, LiveRouteDialect::Realtime),
            Err(_v1_error)
                if realtime_v2_request_is_codex(context.uri.query(), &context.headers) =>
            {
                match direct_realtime_v2_model_from_query(context.uri.query()) {
                    Ok(model) => (model, LiveRouteDialect::RealtimeV2),
                    Err(error) => {
                        return Err(preflight_rejection(
                            context,
                            "direct",
                            error.status_code(),
                            error.code(),
                            error.client_message(),
                        ));
                    }
                }
            }
            Err(error) => {
                // Preserve the V1 error (notably InvalidLiveIntent) for
                // ordinary Realtime callers and malformed Codex V1 queries.
                return Err(preflight_rejection(
                    context,
                    "direct",
                    error.status_code(),
                    error.code(),
                    error.client_message(),
                ));
            }
        };
        return prepare_direct_live_websocket(state, context, client_model.as_str(), dialect)
            .await
            .map(PreparedLiveWebSocket::Direct);
    }
    let (dialect, call_id) =
        match sideband_call_from_request(context.uri.path(), context.uri.query()) {
            Ok(sideband) => sideband,
            Err(error) => {
                return Err(preflight_rejection(
                    context,
                    "sideband",
                    error.status_code(),
                    error.code(),
                    error.client_message(),
                ));
            }
        };
    let Some(auth) = context.decision.auth_context.as_ref() else {
        return Err(preflight_rejection(
            context,
            "sideband",
            StatusCode::UNAUTHORIZED,
            "authentication_required",
            "Authentication required",
        ));
    };
    let registry = LiveCallRegistry::new(std::sync::Arc::clone(&state.runtime_state));
    let lookup = tokio::time::timeout(
        SIDEBAND_LOOKUP_TIMEOUT,
        registry.lookup_with_status(
            auth.user_id.as_str(),
            auth.api_key_id.as_str(),
            call_id.as_str(),
        ),
    )
    .await;
    let record = match lookup {
        Ok(Ok(LiveCallLookup::Found(record))) => record,
        Ok(Ok(LiveCallLookup::Missing)) => {
            info!(
                target: LIVE_LOG_TARGET,
                event_name = "codex_live_sideband_binding_miss",
                log_type = "event",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                "Codex Live sideband binding was not found"
            );
            return Err(preflight_rejection(
                context,
                "sideband",
                StatusCode::NOT_FOUND,
                "sideband_binding_missing",
                "Codex Live call binding was not found",
            ));
        }
        Ok(Ok(LiveCallLookup::Expired)) => {
            info!(
                target: LIVE_LOG_TARGET,
                event_name = "codex_live_sideband_binding_expired",
                log_type = "event",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                "Codex Live sideband binding has expired"
            );
            return Err(preflight_rejection(
                context,
                "sideband",
                StatusCode::GONE,
                "sideband_binding_expired",
                "Codex Live call binding has expired",
            ));
        }
        Ok(Err(error)) => {
            log_registry_error(context, &error);
            return Err(preflight_rejection(
                context,
                "sideband",
                StatusCode::SERVICE_UNAVAILABLE,
                "sideband_binding_unavailable",
                "Codex Live sideband binding is temporarily unavailable",
            ));
        }
        Err(_) => {
            return Err(preflight_rejection(
                context,
                "sideband",
                StatusCode::SERVICE_UNAVAILABLE,
                "sideband_binding_lookup_timeout",
                "Timed out loading the Codex Live sideband binding",
            ));
        }
    };
    prepare_sideband_live_websocket(state, context, call_id, record, dialect)
        .await
        .map(PreparedLiveWebSocket::Sideband)
}

async fn prepare_direct_live_websocket(
    state: &AppState,
    context: &WebSocketRequestContext,
    client_model: &str,
    dialect: LiveRouteDialect,
) -> Result<PreparedLiveRelay, LiveWebSocketPreflightRejection> {
    let started_at = Instant::now();
    let candidate = match plan_live_candidate(
        state,
        context.trace_id.as_str(),
        &context.decision,
        &context.headers,
        &context.remote_addr,
        client_model,
        dialect,
        None,
        None,
    )
    .await
    {
        Ok(LiveCandidatePlanningOutcome {
            candidate: Some(candidate),
            ..
        }) => candidate,
        Ok(LiveCandidatePlanningOutcome {
            candidate: None,
            runtime_miss,
        }) => {
            log_live_candidate_unavailable(context, "direct", client_model, runtime_miss.as_ref());
            return Err(preflight_rejection(
                context,
                "direct",
                StatusCode::SERVICE_UNAVAILABLE,
                "candidate_unavailable",
                "No eligible Codex Live provider mapping is available",
            ));
        }
        Err(error) => {
            warn!(
                target: LIVE_LOG_TARGET,
                event_name = "codex_live_planning_failed",
                log_type = "ops",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                mode = "direct",
                error_kind = gateway_error_kind(&error),
                "Codex Live direct candidate planning failed"
            );
            return Err(preflight_rejection(
                context,
                "direct",
                gateway_error_status(&error),
                "planning_failed",
                "Codex Live provider planning failed",
            ));
        }
    };
    let pool_lease = LivePoolLeaseGuard::new(state, &candidate);
    let upstream_url = match direct_live_websocket_url_for_dialect(&candidate, dialect) {
        Ok(url) => url,
        Err(error) => {
            pool_lease.release().await;
            return Err(preflight_rejection(
                context,
                "direct",
                error.status_code(),
                error.code(),
                error.client_message(),
            ));
        }
    };
    let LiveRelayAdmission {
        capacity: admission,
        audit,
    } = match acquire_live_relay_admission(
        state,
        context,
        &candidate,
        upstream_url.clone(),
        LiveAuditTransport::DirectWebSocket,
    )
    .await
    {
        Ok(admission) => admission,
        Err(failure) => {
            pool_lease.release().await;
            let status = failure.error.status();
            let termination = failure.error.termination();
            return Err(audited_preflight_rejection(
                state,
                context,
                "direct",
                status,
                termination,
                failure.error.client_message(),
                started_at,
                failure.audit,
            )
            .await);
        }
    };
    let provider_id = candidate.execution.provider_id.clone().unwrap_or_default();
    let endpoint_id = candidate.execution.endpoint_id.clone().unwrap_or_default();
    let key_id = candidate.execution.key_id.clone().unwrap_or_default();
    let provider_model = candidate.provider_model.clone();
    if !pool_lease.is_healthy() {
        admission.release().await;
        pool_lease.release().await;
        return Err(audited_preflight_rejection(
            state,
            context,
            "direct",
            StatusCode::SERVICE_UNAVAILABLE,
            "pool_key_lease_lost",
            "Codex Live provider ownership was lost",
            started_at,
            Some(audit),
        )
        .await);
    }
    let mut execution = candidate.execution;
    execution.upstream_url = Some(upstream_url);
    let mut upstream = match connect_upstream_websocket(
        &execution,
        LIVE_WEBSOCKET_SESSION_LIMITS,
        LIVE_UPSTREAM_ERRORS,
    )
    .await
    {
        Ok(connection) => connection.socket,
        Err(error_code) => {
            warn!(
                target: LIVE_LOG_TARGET,
                event_name = "codex_live_upstream_connect_failed",
                log_type = "ops",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                provider_id = %provider_id,
                endpoint_id = %endpoint_id,
                key_id = %key_id,
                mode = "direct",
                error_code,
                "Codex Live direct upstream connection failed"
            );
            admission.release().await;
            pool_lease.release().await;
            return Err(audited_preflight_rejection(
                state,
                context,
                "direct",
                StatusCode::BAD_GATEWAY,
                "upstream_connect_failed",
                "Codex Live upstream WebSocket connection failed",
                started_at,
                Some(audit),
            )
            .await);
        }
    };
    if !pool_lease.is_healthy() {
        close_upstream_socket(&mut upstream, None).await;
        admission.release().await;
        pool_lease.release().await;
        return Err(audited_preflight_rejection(
            state,
            context,
            "direct",
            StatusCode::SERVICE_UNAVAILABLE,
            "pool_key_lease_lost",
            "Codex Live provider ownership was lost",
            started_at,
            Some(audit),
        )
        .await);
    }
    Ok(PreparedLiveRelay {
        upstream,
        admission,
        audit,
        pool_lease,
        provider_id,
        endpoint_id,
        key_id,
        provider_model,
    })
}

async fn prepare_sideband_live_websocket(
    state: &AppState,
    context: &WebSocketRequestContext,
    call_id: String,
    record: LiveCallRecord,
    dialect: LiveRouteDialect,
) -> Result<PreparedLiveSideband, LiveWebSocketPreflightRejection> {
    let started_at = Instant::now();
    let binding = record.binding();
    let Some(auth) = context.decision.auth_context.as_ref() else {
        return Err(preflight_rejection(
            context,
            "sideband",
            StatusCode::UNAUTHORIZED,
            "authentication_required",
            "Authentication required",
        ));
    };
    let registry = LiveCallRegistry::new(std::sync::Arc::clone(&state.runtime_state));
    let mut sideband_lease = match tokio::time::timeout(
        SIDEBAND_LOOKUP_TIMEOUT,
        registry.acquire_sideband_attachment(
            auth.user_id.as_str(),
            auth.api_key_id.as_str(),
            call_id.as_str(),
        ),
    )
    .await
    {
        Ok(Ok(lease)) => lease,
        Ok(Err(LiveCallRegistryError::SidebandAlreadyAttached)) => {
            info!(
                target: LIVE_LOG_TARGET,
                event_name = "codex_live_sideband_attachment_conflict",
                log_type = "event",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                provider_id = %binding.pinned_candidate().provider_id(),
                endpoint_id = %binding.pinned_candidate().endpoint_id(),
                key_id = %binding.pinned_candidate().key_id(),
                "Codex Live call already has an active sideband attachment"
            );
            return Err(preflight_rejection(
                context,
                "sideband",
                StatusCode::CONFLICT,
                "sideband_attachment_conflict",
                "Codex Live call already has an active sideband connection",
            ));
        }
        Ok(Err(error)) => {
            log_sideband_lease_error(context, &error, "acquire");
            return Err(preflight_rejection(
                context,
                "sideband",
                StatusCode::SERVICE_UNAVAILABLE,
                "sideband_attachment_unavailable",
                "Codex Live sideband ownership is temporarily unavailable",
            ));
        }
        Err(_) => {
            return Err(preflight_rejection(
                context,
                "sideband",
                StatusCode::SERVICE_UNAVAILABLE,
                "sideband_attachment_timeout",
                "Timed out acquiring Codex Live sideband ownership",
            ));
        }
    };

    let planned_candidate = while_sideband_lease_healthy(
        &sideband_lease,
        plan_live_candidate(
            state,
            context.trace_id.as_str(),
            &context.decision,
            &context.headers,
            &context.remote_addr,
            binding.client_model(),
            dialect,
            Some(binding.pinned_candidate()),
            record.provider_outbound_context(),
        ),
    )
    .await;
    let candidate = match planned_candidate {
        Err(loss) => {
            release_sideband_lease(&mut sideband_lease, context).await;
            return Err(preflight_rejection(
                context,
                "sideband",
                StatusCode::SERVICE_UNAVAILABLE,
                sideband_loss_termination(loss),
                sideband_loss_message(loss),
            ));
        }
        Ok(Ok(LiveCandidatePlanningOutcome {
            candidate: Some(candidate),
            ..
        })) if record.matches_candidate(&candidate) => candidate,
        Ok(Ok(LiveCandidatePlanningOutcome {
            candidate: Some(candidate),
            ..
        })) => {
            crate::orchestration::release_pool_key_lease_from_report_context(
                state,
                candidate.execution.report_context.as_ref(),
            )
            .await;
            release_sideband_lease(&mut sideband_lease, context).await;
            return Err(preflight_rejection(
                context,
                "sideband",
                StatusCode::GONE,
                "sideband_binding_changed",
                "Codex Live call provider binding is no longer valid",
            ));
        }
        Ok(Ok(LiveCandidatePlanningOutcome {
            candidate: None,
            runtime_miss,
        })) => {
            log_live_candidate_unavailable(
                context,
                "sideband",
                binding.client_model(),
                runtime_miss.as_ref(),
            );
            release_sideband_lease(&mut sideband_lease, context).await;
            return Err(preflight_rejection(
                context,
                "sideband",
                StatusCode::GONE,
                "sideband_binding_disabled",
                "Codex Live call provider key or model is no longer available",
            ));
        }
        Ok(Err(error)) => {
            warn!(
                target: LIVE_LOG_TARGET,
                event_name = "codex_live_sideband_planning_failed",
                log_type = "ops",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                error_kind = gateway_error_kind(&error),
                "Codex Live sideband pinned candidate validation failed"
            );
            release_sideband_lease(&mut sideband_lease, context).await;
            return Err(preflight_rejection(
                context,
                "sideband",
                gateway_error_status(&error),
                "planning_failed",
                "Codex Live provider validation failed",
            ));
        }
    };
    let pool_lease = LivePoolLeaseGuard::new(state, &candidate);
    let upstream_url = match live_sideband_url(&candidate, call_id.as_str(), dialect) {
        Ok(url) => url,
        Err(error) => {
            release_sideband_lease(&mut sideband_lease, context).await;
            pool_lease.release().await;
            return Err(preflight_rejection(
                context,
                "sideband",
                error.status_code(),
                error.code(),
                error.client_message(),
            ));
        }
    };
    let admission_result = while_sideband_lease_healthy(
        &sideband_lease,
        acquire_live_relay_admission(
            state,
            context,
            &candidate,
            upstream_url.clone(),
            LiveAuditTransport::Sideband,
        ),
    )
    .await;
    let LiveRelayAdmission {
        capacity: admission,
        audit,
    } = match admission_result {
        Err(loss) => {
            release_sideband_lease(&mut sideband_lease, context).await;
            pool_lease.release().await;
            return Err(preflight_rejection(
                context,
                "sideband",
                StatusCode::SERVICE_UNAVAILABLE,
                sideband_loss_termination(loss),
                sideband_loss_message(loss),
            ));
        }
        Ok(Ok(admission)) => admission,
        Ok(Err(failure)) => {
            release_sideband_lease(&mut sideband_lease, context).await;
            pool_lease.release().await;
            let status = failure.error.status();
            let termination = failure.error.termination();
            return Err(audited_preflight_rejection(
                state,
                context,
                "sideband",
                status,
                termination,
                failure.error.client_message(),
                started_at,
                failure.audit,
            )
            .await);
        }
    };
    let provider_id = candidate.execution.provider_id.clone().unwrap_or_default();
    let endpoint_id = candidate.execution.endpoint_id.clone().unwrap_or_default();
    let key_id = candidate.execution.key_id.clone().unwrap_or_default();
    let provider_model = candidate.provider_model.clone();
    if !pool_lease.is_healthy() {
        admission.release().await;
        release_sideband_lease(&mut sideband_lease, context).await;
        pool_lease.release().await;
        return Err(audited_preflight_rejection(
            state,
            context,
            "sideband",
            StatusCode::SERVICE_UNAVAILABLE,
            "pool_key_lease_lost",
            "Codex Live provider ownership was lost",
            started_at,
            Some(audit),
        )
        .await);
    }
    let mut execution = candidate.execution;
    execution.upstream_url = Some(upstream_url);
    let upstream_connection = while_sideband_lease_healthy(
        &sideband_lease,
        connect_upstream_websocket(
            &execution,
            LIVE_WEBSOCKET_SESSION_LIMITS,
            LIVE_UPSTREAM_ERRORS,
        ),
    )
    .await;
    let mut upstream = match upstream_connection {
        Err(loss) => {
            release_sideband_lease(&mut sideband_lease, context).await;
            admission.release().await;
            pool_lease.release().await;
            return Err(audited_preflight_rejection(
                state,
                context,
                "sideband",
                StatusCode::SERVICE_UNAVAILABLE,
                sideband_loss_termination(loss),
                sideband_loss_message(loss),
                started_at,
                Some(audit),
            )
            .await);
        }
        Ok(Ok(connection)) => connection.socket,
        Ok(Err(error_code)) => {
            warn!(
                target: LIVE_LOG_TARGET,
                event_name = "codex_live_sideband_connect_failed",
                log_type = "ops",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                trace_id = %context.trace_id,
                provider_id = %provider_id,
                endpoint_id = %endpoint_id,
                key_id = %key_id,
                error_code,
                "Codex Live sideband upstream connection failed"
            );
            release_sideband_lease(&mut sideband_lease, context).await;
            admission.release().await;
            pool_lease.release().await;
            return Err(audited_preflight_rejection(
                state,
                context,
                "sideband",
                StatusCode::BAD_GATEWAY,
                "upstream_connect_failed",
                "Codex Live sideband connection failed",
                started_at,
                Some(audit),
            )
            .await);
        }
    };
    if !pool_lease.is_healthy() {
        close_upstream_socket(&mut upstream, None).await;
        admission.release().await;
        release_sideband_lease(&mut sideband_lease, context).await;
        pool_lease.release().await;
        return Err(audited_preflight_rejection(
            state,
            context,
            "sideband",
            StatusCode::SERVICE_UNAVAILABLE,
            "pool_key_lease_lost",
            "Codex Live provider ownership was lost",
            started_at,
            Some(audit),
        )
        .await);
    }
    Ok(PreparedLiveSideband {
        relay: PreparedLiveRelay {
            upstream,
            admission,
            audit,
            pool_lease,
            provider_id,
            endpoint_id,
            key_id,
            provider_model,
        },
        sideband_lease,
    })
}

pub(super) async fn run_live_websocket(
    mut client_socket: WebSocket,
    state: AppState,
    context: WebSocketRequestContext,
    prepared: PreparedLiveWebSocket,
) {
    let connection_log = WebSocketConnectionLog::new(&context, LIVE_CONNECTION_LOG_SPEC);
    connection_log.log_opened();
    match prepared {
        PreparedLiveWebSocket::Direct(prepared) => {
            run_direct(&mut client_socket, &state, &context, prepared).await
        }
        PreparedLiveWebSocket::Sideband(prepared) => {
            run_sideband(&mut client_socket, &state, &context, prepared).await
        }
    }
}

async fn run_direct(
    client_socket: &mut WebSocket,
    state: &AppState,
    context: &WebSocketRequestContext,
    prepared: PreparedLiveRelay,
) {
    let session_started_at = Instant::now();
    let PreparedLiveRelay {
        mut upstream,
        admission,
        audit,
        pool_lease,
        provider_id,
        endpoint_id,
        key_id,
        provider_model,
    } = prepared;
    let initial = match read_initial_session_update(client_socket).await {
        Ok(Some(initial)) => initial,
        Ok(None) => {
            close_upstream_socket(&mut upstream, None).await;
            admission.release().await;
            pool_lease.release().await;
            audit
                .finish(
                    state,
                    live_terminal_from_relay(
                        "client_closed",
                        elapsed_ms(session_started_at),
                        RelayStats::default(),
                    ),
                )
                .await;
            return;
        }
        Err(error) => {
            send_live_error(
                client_socket,
                error.status_code().as_u16(),
                error.code(),
                error.client_message(),
            )
            .await;
            close_client_socket(
                client_socket,
                if error.is_timeout() {
                    CLOSE_TRY_AGAIN
                } else {
                    CLOSE_POLICY_VIOLATION
                },
                "invalid initial Live event",
            )
            .await;
            close_upstream_socket(&mut upstream, None).await;
            admission.release().await;
            pool_lease.release().await;
            audit
                .finish(
                    state,
                    LiveSessionTerminal::failure(
                        error.status_code().as_u16(),
                        error.code(),
                        elapsed_ms(session_started_at),
                    ),
                )
                .await;
            return;
        }
    };
    let initial =
        rewrite_live_session_model(initial.as_str(), provider_model.as_str()).unwrap_or(initial);
    let initial_bytes = initial.len() as u64;
    if send_upstream_message(&mut upstream, WreqWsMessage::Text(initial.into()))
        .await
        .is_err()
    {
        close_upstream_socket(&mut upstream, None).await;
        admission.release().await;
        pool_lease.release().await;
        close_client_socket(client_socket, CLOSE_TRY_AGAIN, "Live upstream write failed").await;
        let mut terminal = LiveSessionTerminal::failure(
            502,
            "initial_upstream_write_failed",
            elapsed_ms(session_started_at),
        );
        terminal.client_frames = 1;
        terminal.client_bytes = initial_bytes;
        audit.finish(state, terminal).await;
        return;
    }
    let terminal = relay_live(
        client_socket,
        &mut upstream,
        context,
        "direct",
        provider_id.as_str(),
        endpoint_id.as_str(),
        key_id.as_str(),
        provider_model.as_str(),
        &pool_lease,
        None,
        RelayStats {
            client_frames: 1,
            client_bytes: initial_bytes,
            ..RelayStats::default()
        },
        session_started_at,
    )
    .await;
    let close_client = matches!(
        terminal.termination,
        "connection_duration_limit" | "connection_admission_lost"
    );
    close_upstream_socket(&mut upstream, None).await;
    admission.release().await;
    pool_lease.release().await;
    if close_client {
        close_client_socket(client_socket, CLOSE_TRY_AGAIN, terminal.termination).await;
    }
    audit.finish(state, terminal).await;
}

async fn run_sideband(
    client_socket: &mut WebSocket,
    state: &AppState,
    context: &WebSocketRequestContext,
    prepared: PreparedLiveSideband,
) {
    let PreparedLiveSideband {
        relay:
            PreparedLiveRelay {
                mut upstream,
                admission,
                audit,
                pool_lease,
                provider_id,
                endpoint_id,
                key_id,
                provider_model,
            },
        mut sideband_lease,
    } = prepared;
    // A sideband attaches to an already-created WebRTC session. Sending a
    // second synthetic `session.update` here would corrupt the protocol.
    let session_started_at = Instant::now();
    let terminal = relay_live(
        client_socket,
        &mut upstream,
        context,
        "sideband",
        provider_id.as_str(),
        endpoint_id.as_str(),
        key_id.as_str(),
        provider_model.as_str(),
        &pool_lease,
        Some(&sideband_lease),
        RelayStats::default(),
        session_started_at,
    )
    .await;
    let close_client = matches!(
        terminal.termination,
        "connection_duration_limit" | "connection_admission_lost"
    );
    close_upstream_socket(&mut upstream, None).await;
    release_sideband_lease(&mut sideband_lease, context).await;
    admission.release().await;
    pool_lease.release().await;
    if close_client {
        close_client_socket(client_socket, CLOSE_TRY_AGAIN, terminal.termination).await;
    }
    audit.finish(state, terminal).await;
}

async fn acquire_live_relay_admission(
    state: &AppState,
    context: &WebSocketRequestContext,
    candidate: &PlannedLiveCandidate,
    upstream_url: String,
    transport: LiveAuditTransport,
) -> Result<LiveRelayAdmission, LiveRelayAdmissionFailure> {
    let attempt = match build_live_stream_admission_attempt(
        candidate,
        &context.headers,
        &context.remote_addr,
        upstream_url,
    ) {
        Ok(Some(attempt)) => attempt,
        Ok(None) => {
            return Err(LiveRelayAdmissionFailure {
                error: LiveRelayAdmissionError::PlanUnavailable,
                audit: None,
            });
        }
        Err(error) => {
            return Err(LiveRelayAdmissionFailure {
                error: LiveRelayAdmissionError::Gateway(error),
                audit: None,
            });
        }
    };
    let audit = LiveSessionAudit::from_attempt(&attempt, transport);
    let balance_rejection = execution_plan_balance_capacity_rejection(
        state,
        &context.decision,
        &attempt.plan,
        attempt.report_context.as_ref(),
    )
    .await;
    match balance_rejection {
        Ok(None) => {}
        Ok(Some(_)) => {
            return Err(LiveRelayAdmissionFailure {
                error: LiveRelayAdmissionError::BalanceRejected,
                audit: Some(audit),
            });
        }
        Err(error) => {
            return Err(LiveRelayAdmissionFailure {
                error: LiveRelayAdmissionError::Gateway(error),
                audit: Some(audit),
            });
        }
    }
    match ResponsesWebSocketTurnAdmission::acquire(state, &attempt.plan, context.trace_id.as_str())
        .await
    {
        Ok(capacity) => Ok(LiveRelayAdmission { capacity, audit }),
        Err(error) => Err(LiveRelayAdmissionFailure {
            error: LiveRelayAdmissionError::Gateway(error),
            audit: Some(audit),
        }),
    }
}

async fn audited_preflight_rejection(
    state: &AppState,
    context: &WebSocketRequestContext,
    mode: &'static str,
    status: StatusCode,
    termination: &'static str,
    message: &'static str,
    started_at: Instant,
    audit: Option<LiveSessionAudit>,
) -> LiveWebSocketPreflightRejection {
    let audit_persisted = audit.is_some();
    if let Some(audit) = audit {
        audit
            .finish(
                state,
                LiveSessionTerminal::failure(status.as_u16(), termination, elapsed_ms(started_at)),
            )
            .await;
    }
    let mut rejection = preflight_rejection(context, mode, status, termination, message);
    rejection.audit_persisted = audit_persisted;
    rejection
}

fn preflight_rejection(
    context: &WebSocketRequestContext,
    mode: &'static str,
    status: StatusCode,
    termination: &'static str,
    message: &'static str,
) -> LiveWebSocketPreflightRejection {
    warn!(
        target: LIVE_LOG_TARGET,
        event_name = "codex_live_websocket_preflight_rejected",
        log_type = "event",
        transport = WEBSOCKET_LOG_TRANSPORT,
        websocket = true,
        trace_id = %context.trace_id,
        mode,
        status_code = status.as_u16(),
        termination,
        "Codex Live WebSocket preflight rejected the HTTP upgrade"
    );
    LiveWebSocketPreflightRejection {
        status,
        termination,
        message,
        audit_persisted: false,
    }
}

fn log_live_candidate_unavailable(
    context: &WebSocketRequestContext,
    mode: &'static str,
    client_model: &str,
    runtime_miss: Option<&LocalExecutionRuntimeMissDiagnostic>,
) {
    warn!(
        target: LIVE_LOG_TARGET,
        event_name = "codex_live_candidate_unavailable",
        log_type = "ops",
        transport = WEBSOCKET_LOG_TRANSPORT,
        websocket = true,
        trace_id = %context.trace_id,
        mode,
        client_model,
        runtime_miss_reason = runtime_miss
            .map(|diagnostic| diagnostic.reason.as_str())
            .unwrap_or("unknown"),
        candidate_count = runtime_miss
            .and_then(|diagnostic| diagnostic.candidate_count)
            .unwrap_or(0),
        skipped_candidate_count = runtime_miss
            .and_then(|diagnostic| diagnostic.skipped_candidate_count)
            .unwrap_or(0),
        skip_reasons = runtime_miss
            .and_then(|diagnostic| diagnostic.skip_reasons_summary())
            .unwrap_or_default(),
        "Codex Live request has no eligible provider mapping"
    );
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

fn gateway_error_status(error: &GatewayError) -> StatusCode {
    match error {
        GatewayError::UpstreamUnavailable { .. } | GatewayError::ControlUnavailable { .. } => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        GatewayError::LocalExecutionPlanningTimeout { .. } => StatusCode::GATEWAY_TIMEOUT,
        GatewayError::AdmissionTimeout { .. } => StatusCode::TOO_MANY_REQUESTS,
        GatewayError::PlanUsageLimited(_) => StatusCode::TOO_MANY_REQUESTS,
        GatewayError::LastActiveAdminUpdateDenied | GatewayError::LastActiveAdminDeleteDenied => {
            StatusCode::BAD_REQUEST
        }
        GatewayError::Client { status, .. } => *status,
        GatewayError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

const fn sideband_loss_termination(loss: LiveSidebandLeaseLoss) -> &'static str {
    match loss {
        LiveSidebandLeaseLoss::OwnershipLost => "sideband_attachment_lease_lost",
        LiveSidebandLeaseLoss::StorageUnavailable => "sideband_attachment_lease_renewal_failed",
    }
}

const fn sideband_loss_message(loss: LiveSidebandLeaseLoss) -> &'static str {
    match loss {
        LiveSidebandLeaseLoss::OwnershipLost => "Codex Live sideband ownership was lost",
        LiveSidebandLeaseLoss::StorageUnavailable => {
            "Codex Live sideband ownership could not be renewed"
        }
    }
}

fn elapsed_ms(started_at: Instant) -> u64 {
    started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

async fn reject_lost_pool_lease(
    client_socket: &mut WebSocket,
    context: &WebSocketRequestContext,
    mode: &'static str,
    provider_id: &str,
    endpoint_id: &str,
    key_id: &str,
) {
    warn!(
        target: LIVE_LOG_TARGET,
        event_name = "codex_live_pool_key_lease_lost",
        log_type = "ops",
        transport = WEBSOCKET_LOG_TRANSPORT,
        websocket = true,
        trace_id = %context.trace_id,
        provider_id,
        endpoint_id,
        key_id,
        mode,
        "Codex Live relay stopped after losing its scheduler pool-key lease"
    );
    send_live_error(
        client_socket,
        503,
        "codex_live_pool_key_lease_lost",
        "Codex Live provider ownership was lost",
    )
    .await;
    close_client_socket(
        client_socket,
        CLOSE_TRY_AGAIN,
        "Live provider ownership lost",
    )
    .await;
}

async fn read_initial_session_update(
    client_socket: &mut WebSocket,
) -> Result<Option<String>, super::protocol::LiveProtocolError> {
    tokio::time::timeout(
        LIVE_WEBSOCKET_SESSION_LIMITS.initial_message_timeout,
        async {
            loop {
                let Some(message) = client_socket.next().await else {
                    return Ok(None);
                };
                let message = message
                    .map_err(|_| super::protocol::LiveProtocolError::InitialClientReadFailed)?;
                match message {
                    AxumWsMessage::Text(text) => {
                        validate_initial_session_update(text.as_str())?;
                        return Ok(Some(text.to_string()));
                    }
                    AxumWsMessage::Ping(payload) => {
                        send_client_message(client_socket, AxumWsMessage::Pong(payload))
                            .await
                            .map_err(|_| {
                                super::protocol::LiveProtocolError::InitialClientReadFailed
                            })?;
                    }
                    AxumWsMessage::Pong(_) => {}
                    AxumWsMessage::Close(_) => return Ok(None),
                    AxumWsMessage::Binary(_) => {
                        return Err(super::protocol::LiveProtocolError::InitialEventMustBeText)
                    }
                }
            }
        },
    )
    .await
    .map_err(|_| super::protocol::LiveProtocolError::InitialSessionUpdateTimeout)?
}

#[derive(Clone, Copy, Default)]
struct RelayStats {
    client_frames: u64,
    client_bytes: u64,
    upstream_frames: u64,
    upstream_bytes: u64,
    first_upstream_frame_ms: Option<u64>,
}

async fn relay_live(
    client_socket: &mut WebSocket,
    upstream: &mut wreq::ws::WebSocket,
    context: &WebSocketRequestContext,
    mode: &'static str,
    provider_id: &str,
    endpoint_id: &str,
    key_id: &str,
    provider_model: &str,
    pool_lease: &LivePoolLeaseGuard,
    sideband_lease: Option<&LiveSidebandLease>,
    stats: RelayStats,
    started_at: Instant,
) -> LiveSessionTerminal {
    let connection_deadline = tokio::time::sleep_until(tokio::time::Instant::from_std(
        started_at + LIVE_WEBSOCKET_SESSION_LIMITS.max_connection_duration,
    ));
    tokio::pin!(connection_deadline);
    let mut pool_lease_health = tokio::time::interval(Duration::from_secs(1));
    pool_lease_health.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let stats = Arc::new(Mutex::new(stats));
    let session_close = Arc::new(tokio::sync::Notify::new());
    let relay_control = WebSocketRelayPumpControl::new();

    let termination = {
        let (mut client_write, mut client_read) = (&mut *client_socket).split();
        let (mut upstream_write, mut upstream_read) = (&mut *upstream).split();

        let client_to_upstream = {
            let control = relay_control.clone();
            let stats = Arc::clone(&stats);
            let session_close = Arc::clone(&session_close);
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
                        let (bytes, is_close, is_session_close) = client_frame_metadata(&client);
                        {
                            let mut stats = stats
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            stats.client_frames = stats.client_frames.saturating_add(1);
                            stats.client_bytes = stats.client_bytes.saturating_add(bytes as u64);
                        }
                        let client = match client {
                            AxumWsMessage::Text(text) => {
                                rewrite_live_session_model(text.as_str(), provider_model)
                                    .map_or(AxumWsMessage::Text(text), |rewritten| {
                                        AxumWsMessage::Text(rewritten.into())
                                    })
                            }
                            other => other,
                        };
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
                        if is_session_close {
                            session_close.notify_one();
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
        let mut close_deadline = None;
        let termination = loop {
            tokio::select! {
                termination = &mut client_to_upstream => break termination,
                termination = &mut upstream_to_client => break termination,
                _ = &mut connection_deadline => break "connection_duration_limit",
                _ = wait_for_connection_permit_loss(context.websocket_connection_permit.as_ref()) => {
                    break "connection_admission_lost";
                }
                _ = pool_lease_health.tick() => {
                    if !pool_lease.is_healthy() {
                        break "pool_key_lease_lost";
                    }
                }
                _ = session_close.notified(), if close_deadline.is_none() => {
                    close_deadline = Some(Instant::now() + SESSION_CLOSE_DRAIN_TIMEOUT);
                }
                _ = wait_for_optional_deadline(close_deadline) => {
                    break "session_close_drain_timeout";
                }
                loss = wait_for_sideband_lease_loss(sideband_lease) => {
                    break match loss {
                        LiveSidebandLeaseLoss::OwnershipLost => "sideband_attachment_lease_lost",
                        LiveSidebandLeaseLoss::StorageUnavailable => {
                            "sideband_attachment_lease_renewal_failed"
                        }
                    };
                }
            }
        };
        relay_control.cancel();
        termination
    };
    match termination {
        "pool_key_lease_lost" => {
            reject_lost_pool_lease(
                client_socket,
                context,
                mode,
                provider_id,
                endpoint_id,
                key_id,
            )
            .await;
        }
        "sideband_attachment_lease_lost" => {
            reject_sideband_lease_loss(
                client_socket,
                context,
                LiveSidebandLeaseLoss::OwnershipLost,
            )
            .await;
        }
        "sideband_attachment_lease_renewal_failed" => {
            reject_sideband_lease_loss(
                client_socket,
                context,
                LiveSidebandLeaseLoss::StorageUnavailable,
            )
            .await;
        }
        _ => {}
    }
    let stats = *stats
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let elapsed_ms = started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    info!(
        target: LIVE_LOG_TARGET,
        event_name = "codex_live_relay_finished",
        log_type = "event",
        transport = WEBSOCKET_LOG_TRANSPORT,
        websocket = true,
        trace_id = %context.trace_id,
        provider_id,
        endpoint_id,
        key_id,
        mode,
        termination,
        client_frames = stats.client_frames,
        client_bytes = stats.client_bytes,
        upstream_frames = stats.upstream_frames,
        upstream_bytes = stats.upstream_bytes,
        elapsed_ms,
        usage_unavailable = true,
        "Codex Live opaque relay finished"
    );
    live_terminal_from_relay(termination, elapsed_ms, stats)
}

fn live_terminal_from_relay(
    termination: &'static str,
    elapsed_ms: u64,
    stats: RelayStats,
) -> LiveSessionTerminal {
    let (disposition, status_code) = match termination {
        "client_close_frame" | "upstream_close_frame" | "session_close_drain_timeout" => {
            (LiveSessionDisposition::Completed, 200)
        }
        "client_closed"
        | "client_read_failed"
        | "client_write_failed"
        | "connection_duration_limit" => (LiveSessionDisposition::Cancelled, 499),
        "pool_key_lease_lost"
        | "connection_admission_lost"
        | "sideband_attachment_lease_lost"
        | "sideband_attachment_lease_renewal_failed" => (LiveSessionDisposition::Failed, 503),
        "upstream_closed" | "upstream_read_failed" | "upstream_write_failed" => {
            (LiveSessionDisposition::Failed, 502)
        }
        _ => (LiveSessionDisposition::Failed, 500),
    };
    LiveSessionTerminal {
        disposition,
        status_code,
        termination,
        elapsed_ms,
        first_upstream_frame_ms: stats.first_upstream_frame_ms,
        client_frames: stats.client_frames,
        client_bytes: stats.client_bytes,
        upstream_frames: stats.upstream_frames,
        upstream_bytes: stats.upstream_bytes,
    }
}

async fn while_sideband_lease_healthy<T, F>(
    lease: &LiveSidebandLease,
    operation: F,
) -> Result<T, LiveSidebandLeaseLoss>
where
    F: Future<Output = T>,
{
    if let Some(loss) = lease.loss() {
        return Err(loss);
    }
    tokio::select! {
        loss = lease.wait_for_loss() => Err(loss),
        output = operation => lease.loss().map_or(Ok(output), Err),
    }
}

async fn wait_for_sideband_lease_loss(lease: Option<&LiveSidebandLease>) -> LiveSidebandLeaseLoss {
    match lease {
        Some(lease) => lease.wait_for_loss().await,
        None => std::future::pending().await,
    }
}

async fn reject_sideband_lease_loss(
    client_socket: &mut WebSocket,
    context: &WebSocketRequestContext,
    loss: LiveSidebandLeaseLoss,
) {
    warn!(
        target: LIVE_LOG_TARGET,
        event_name = "codex_live_sideband_lease_lost",
        log_type = "ops",
        transport = WEBSOCKET_LOG_TRANSPORT,
        websocket = true,
        trace_id = %context.trace_id,
        error_kind = loss.kind(),
        "Codex Live sideband attachment lease was lost"
    );
    let (code, message, close_reason) = match loss {
        LiveSidebandLeaseLoss::OwnershipLost => (
            "codex_live_sideband_lease_lost",
            "Codex Live sideband ownership was lost",
            "Live sideband ownership lost",
        ),
        LiveSidebandLeaseLoss::StorageUnavailable => (
            "codex_live_sideband_lease_unavailable",
            "Codex Live sideband ownership could not be renewed",
            "Live sideband ownership renewal failed",
        ),
    };
    send_live_error(client_socket, 503, code, message).await;
    close_client_socket(client_socket, CLOSE_TRY_AGAIN, close_reason).await;
}

/// Rewrites only the routing-authoritative model field while keeping the
/// evolving Frameless event schema opaque. Invalid JSON, non-session events,
/// and session updates without an explicit model are forwarded byte-for-byte.
fn rewrite_live_session_model(raw: &str, provider_model: &str) -> Option<String> {
    let mut event: Value = serde_json::from_str(raw).ok()?;
    if event.get("type").and_then(Value::as_str) != Some("session.update") {
        return None;
    }
    let session = event.get_mut("session")?.as_object_mut()?;
    let model = session.get_mut("model")?;
    *model = Value::String(provider_model.to_string());
    serde_json::to_string(&event).ok()
}

fn client_frame_metadata(message: &AxumWsMessage) -> (usize, bool, bool) {
    match message {
        AxumWsMessage::Text(text) => (
            text.len(),
            false,
            event_type(text.as_str()).as_deref() == Some("session.close"),
        ),
        AxumWsMessage::Binary(data) => (data.len(), false, false),
        AxumWsMessage::Ping(data) | AxumWsMessage::Pong(data) => (data.len(), false, false),
        AxumWsMessage::Close(frame) => (
            frame
                .as_ref()
                .map_or(0, |frame| 2usize.saturating_add(frame.reason.len())),
            true,
            false,
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

async fn send_live_error(client_socket: &mut WebSocket, status: u16, code: &str, message: &str) {
    let event = json!({
        "type": "error",
        "status": status,
        "error": {
            "type": "invalid_request_error",
            "code": code,
            "message": message,
        }
    });
    let _ = send_client_message(client_socket, AxumWsMessage::Text(event.to_string().into())).await;
}

fn log_registry_error(context: &WebSocketRequestContext, error: &LiveCallRegistryError) {
    warn!(
        target: LIVE_LOG_TARGET,
        event_name = "codex_live_sideband_binding_lookup_failed",
        log_type = "ops",
        transport = WEBSOCKET_LOG_TRANSPORT,
        websocket = true,
        trace_id = %context.trace_id,
        error_kind = error.kind(),
        "Codex Live sideband binding lookup failed"
    );
}

fn log_sideband_lease_error(
    context: &WebSocketRequestContext,
    error: &LiveCallRegistryError,
    operation: &'static str,
) {
    warn!(
        target: LIVE_LOG_TARGET,
        event_name = "codex_live_sideband_lease_operation_failed",
        log_type = "ops",
        transport = WEBSOCKET_LOG_TRANSPORT,
        websocket = true,
        trace_id = %context.trace_id,
        operation,
        error_kind = error.kind(),
        "Codex Live sideband attachment lease operation failed"
    );
}

async fn release_sideband_lease(lease: &mut LiveSidebandLease, context: &WebSocketRequestContext) {
    match lease.release().await {
        Ok(true) => {}
        Ok(false) => warn!(
            target: LIVE_LOG_TARGET,
            event_name = "codex_live_sideband_lease_release_not_owned",
            log_type = "ops",
            transport = WEBSOCKET_LOG_TRANSPORT,
            websocket = true,
            trace_id = %context.trace_id,
            "Codex Live sideband attachment lease was not owned during release"
        ),
        Err(error) => log_sideband_lease_error(context, &error, "release"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_has_a_distinct_frontdoor_self_loop_error() {
        assert_eq!(
            LIVE_UPSTREAM_ERRORS.frontdoor_self_loop,
            "codex_live_websocket_frontdoor_self_loop"
        );
    }

    #[test]
    fn turn_done_is_opaque_and_does_not_request_connection_close() {
        let message = AxumWsMessage::Text(r#"{"type":"turn.done","future":true}"#.into());
        let (_, is_close, is_session_close) = client_frame_metadata(&message);
        assert!(!is_close);
        assert!(!is_session_close);
    }

    #[test]
    fn only_session_close_starts_the_bounded_drain() {
        let message = AxumWsMessage::Text(r#"{"type":"session.close","future":true}"#.into());
        let (_, is_close, is_session_close) = client_frame_metadata(&message);
        assert!(!is_close);
        assert!(is_session_close);
    }

    #[test]
    fn session_update_model_is_pinned_without_dropping_unknown_fields() {
        let rewritten = rewrite_live_session_model(
            r#"{"type":"session.update","session":{"model":"client-alias","future":true},"event_id":"evt_1","unknown":{"nested":1}}"#,
            "provider-model",
        )
        .expect("session model should be rewritten");
        let value: Value = serde_json::from_str(&rewritten).expect("rewritten JSON");
        assert_eq!(value["session"]["model"], "provider-model");
        assert_eq!(value["session"]["future"], true);
        assert_eq!(value["event_id"], "evt_1");
        assert_eq!(value["unknown"]["nested"], 1);
    }

    #[test]
    fn non_routing_live_frames_remain_opaque() {
        for raw in [
            r#"{"type":"input_audio_buffer.append","audio":"AA=="}"#,
            r#"{"type":"session.update","session":{"future":true}}"#,
            r#"{"type":"session.update","session":null}"#,
            "not-json",
        ] {
            assert_eq!(rewrite_live_session_model(raw, "provider-model"), None);
        }
    }
}
