//! Session-level audit records for Codex Live transports.
//!
//! Frameless Bidi does not expose an authoritative token/cost usage object.
//! These records therefore capture exactly one bounded lifecycle summary per
//! connection and are explicitly void for billing. They never infer tokens,
//! audio duration, or cost from frame sizes.

use std::time::{Duration, Instant};

use aether_ai_serving::{AiStreamAttempt, AiSyncAttempt};
use aether_contracts::ExecutionPlan;
use aether_data_contracts::repository::usage::{
    UsageBodyCaptureState, LIVE_SESSION_METADATA_KEY, USAGE_AVAILABLE_METADATA_KEY,
    USAGE_PRICING_AVAILABLE_METADATA_KEY, WEBSOCKET_MODE_METADATA_KEY,
    WEBSOCKET_TRANSPORT_METADATA_KEY,
};
use aether_usage_runtime::build_usage_event_data_seed;
use serde_json::{json, Map, Value};
use tracing::warn;

use crate::control::GatewayControlDecision;
use crate::state::LocalExecutionRuntimeMissDiagnostic;
use crate::usage::{UsageEvent, UsageEventData, UsageEventType};
use crate::AppState;

const LIVE_AUDIT_WRITE_WAIT: Duration = Duration::from_secs(5);
const LIVE_AUDIT_WRITE_HARD_TIMEOUT: Duration = Duration::from_secs(30);
const LIVE_AUDIT_SCHEMA_VERSION: &str = "1";
const LIVE_AUDIT_LOG_TARGET: &str = "aether_gateway::handlers::proxy::codex_live";
pub(super) const LIVE_CALL_CANDIDATE_UNAVAILABLE_MESSAGE: &str =
    "No eligible Codex Live provider mapping is available";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LiveAuditTransport {
    WebRtc,
    DirectWebSocket,
    Sideband,
}

impl LiveAuditTransport {
    const fn transport(self) -> &'static str {
        match self {
            Self::WebRtc => "webrtc",
            Self::DirectWebSocket => "websocket",
            Self::Sideband => "sideband",
        }
    }

    const fn mode(self) -> &'static str {
        match self {
            Self::WebRtc => "call_create",
            Self::DirectWebSocket => "direct",
            Self::Sideband => "sideband",
        }
    }

    const fn websocket_transport(self) -> Option<&'static str> {
        match self {
            Self::WebRtc => None,
            Self::DirectWebSocket => Some("codex_live_direct"),
            Self::Sideband => Some("codex_live_sideband"),
        }
    }
}

/// Marks the existing synchronous SDP call-create audit row as an unmetered
/// WebRTC control exchange. The media leg bypasses Aether after this request.
pub(super) fn mark_live_call_create_report_context(report_context: &mut Option<Value>) {
    attach_live_base_metadata(report_context, LiveAuditTransport::WebRtc);
}

fn build_live_preflight_event(
    decision: Option<&GatewayControlDecision>,
    request_id: &str,
    request_path: &str,
    client_model: Option<&str>,
    transport: LiveAuditTransport,
    terminal: LiveSessionTerminal,
    diagnostic: Option<&LocalExecutionRuntimeMissDiagnostic>,
) -> UsageEvent {
    let auth = decision.and_then(|decision| decision.auth_context.as_ref());
    let mut request_metadata = attach_terminal_metadata(None, transport, &terminal)
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    request_metadata.insert(
        "trace_id".to_string(),
        Value::String(request_id.to_string()),
    );
    request_metadata.insert(
        "request_path".to_string(),
        Value::String(request_path.to_string()),
    );
    if let Some(api_key_is_standalone) = auth.map(|auth| auth.api_key_is_standalone) {
        request_metadata.insert(
            "api_key_is_standalone".to_string(),
            Value::Bool(api_key_is_standalone),
        );
    }

    let terminal_is_error = terminal.disposition != LiveSessionDisposition::Completed;
    let local_execution_runtime_miss_reason = terminal_is_error.then(|| {
        diagnostic
            .map(|diagnostic| diagnostic.reason.trim())
            .filter(|reason| !reason.is_empty())
            .unwrap_or(terminal.termination)
            .to_string()
    });
    let mut data = UsageEventData {
        user_id: auth.map(|auth| auth.user_id.clone()),
        api_key_id: auth.map(|auth| auth.api_key_id.clone()),
        username: auth.and_then(|auth| auth.username.clone()),
        api_key_name: auth.and_then(|auth| auth.api_key_name.clone()),
        provider_name: "unknown".to_string(),
        model: client_model.unwrap_or("unknown").to_string(),
        request_type: Some("live".to_string()),
        api_format: Some("codex:live".to_string()),
        api_family: Some("codex".to_string()),
        endpoint_kind: Some("live".to_string()),
        endpoint_api_format: Some("codex:live".to_string()),
        provider_api_family: Some("codex".to_string()),
        provider_endpoint_kind: Some("live".to_string()),
        is_stream: Some(transport != LiveAuditTransport::WebRtc),
        status_code: Some(terminal.status_code),
        error_message: terminal_is_error.then(|| terminal.termination.to_string()),
        error_category: terminal_is_error.then(|| preflight_error_category(&terminal).to_string()),
        response_time_ms: Some(terminal.elapsed_ms),
        planner_kind: diagnostic.and_then(|diagnostic| diagnostic.plan_kind.clone()),
        route_family: diagnostic
            .and_then(|diagnostic| diagnostic.route_family.clone())
            .or_else(|| decision.and_then(|decision| decision.route_family.clone())),
        route_kind: diagnostic
            .and_then(|diagnostic| diagnostic.route_kind.clone())
            .or_else(|| decision.and_then(|decision| decision.route_kind.clone())),
        execution_path: Some(
            match transport {
                LiveAuditTransport::WebRtc => "codex_live_call",
                LiveAuditTransport::DirectWebSocket | LiveAuditTransport::Sideband => {
                    "codex_live_websocket_preflight"
                }
            }
            .to_string(),
        ),
        local_execution_runtime_miss_reason,
        request_metadata: Some(Value::Object(request_metadata)),
        ..UsageEventData::default()
    };
    clear_live_call_create_http_capture(&mut data);

    UsageEvent::new(terminal.disposition.event_type(), request_id, data)
}

/// Persists one billing-void row when an authenticated Codex Live WebSocket
/// request fails before status 101. Successful preflight does not call this
/// function and is instead recorded exactly once by [`LiveSessionAudit`] when
/// the upgraded connection terminates.
pub(super) fn record_live_websocket_preflight_failure(
    state: &AppState,
    decision: &GatewayControlDecision,
    request_id: &str,
    request_path: &str,
    request_query: Option<&str>,
    status: http::StatusCode,
    termination: &'static str,
) {
    let transport = if request_path.starts_with("/v1/live/")
        || (request_path == "/v1/realtime" && query_has_parameter(request_query, "call_id"))
    {
        LiveAuditTransport::Sideband
    } else {
        LiveAuditTransport::DirectWebSocket
    };
    let client_model = validated_model_query_value(request_query);
    let event = build_live_preflight_event(
        Some(decision),
        request_id,
        request_path,
        client_model.as_deref(),
        transport,
        LiveSessionTerminal::failure(status.as_u16(), termination, 0),
        None,
    );
    spawn_live_audit_event_detached(state, event, "websocket_preflight");
}

fn query_has_parameter(query: Option<&str>, expected: &str) -> bool {
    url::form_urlencoded::parse(query.unwrap_or_default().as_bytes())
        .any(|(name, _)| name.eq_ignore_ascii_case(expected))
}

fn validated_model_query_value(query: Option<&str>) -> Option<String> {
    let mut model = None;
    for (name, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        if !name.eq_ignore_ascii_case("model") {
            continue;
        }
        if model.is_some() || super::protocol::validate_model(value.as_ref()).is_err() {
            return None;
        }
        model = Some(value.into_owned());
    }
    model
}

fn preflight_error_category(terminal: &LiveSessionTerminal) -> &'static str {
    match terminal.disposition {
        LiveSessionDisposition::Completed => "none",
        LiveSessionDisposition::Cancelled => "client_cancelled",
        LiveSessionDisposition::Failed if terminal.status_code >= 500 => "server_error",
        LiveSessionDisposition::Failed => "client_error",
    }
}

fn attach_live_base_metadata(report_context: &mut Option<Value>, transport: LiveAuditTransport) {
    let object = report_context_object(report_context);
    object.insert(USAGE_AVAILABLE_METADATA_KEY.to_string(), Value::Bool(false));
    object.insert(
        USAGE_PRICING_AVAILABLE_METADATA_KEY.to_string(),
        Value::Bool(false),
    );
    object.insert(
        WEBSOCKET_MODE_METADATA_KEY.to_string(),
        Value::Bool(transport.websocket_transport().is_some()),
    );
    if let Some(websocket_transport) = transport.websocket_transport() {
        object.insert(
            WEBSOCKET_TRANSPORT_METADATA_KEY.to_string(),
            Value::String(websocket_transport.to_string()),
        );
    } else {
        object.remove(WEBSOCKET_TRANSPORT_METADATA_KEY);
    }
    object.insert(
        LIVE_SESSION_METADATA_KEY.to_string(),
        json!({
            "schema_version": LIVE_AUDIT_SCHEMA_VERSION,
            "transport": transport.transport(),
            "mode": transport.mode(),
            "usage_state": "unavailable",
        }),
    );
}

fn report_context_object(report_context: &mut Option<Value>) -> &mut Map<String, Value> {
    if !matches!(report_context, Some(Value::Object(_))) {
        let seed = report_context.take();
        let mut object = Map::new();
        if let Some(seed) = seed.filter(|value| !value.is_null()) {
            object.insert("seed".to_string(), seed);
        }
        *report_context = Some(Value::Object(object));
    }
    report_context
        .as_mut()
        .and_then(Value::as_object_mut)
        .expect("Live audit report context was normalized to an object")
}

pub(super) struct LiveSessionAudit {
    plan: ExecutionPlan,
    report_context: Option<Value>,
    transport: LiveAuditTransport,
}

impl LiveSessionAudit {
    pub(super) fn from_attempt(attempt: &AiStreamAttempt, transport: LiveAuditTransport) -> Self {
        Self::from_parts(&attempt.plan, attempt.report_context.as_ref(), transport)
    }

    pub(super) fn from_sync_attempt(
        attempt: &AiSyncAttempt,
        transport: LiveAuditTransport,
    ) -> Self {
        Self::from_parts(&attempt.plan, attempt.report_context.as_ref(), transport)
    }

    fn from_parts(
        plan: &ExecutionPlan,
        report_context: Option<&Value>,
        transport: LiveAuditTransport,
    ) -> Self {
        let mut report_context = report_context.cloned();
        attach_live_base_metadata(&mut report_context, transport);
        Self {
            plan: plan.clone(),
            report_context,
            transport,
        }
    }

    /// Persists one terminal lifecycle row. The spawned write remains alive,
    /// up to its hard timeout, if the bounded caller wait elapses. Closing a
    /// socket therefore does not silently cancel the only audit write, while a
    /// stalled database cannot retain the task forever.
    pub(super) async fn finish(self, state: &AppState, terminal: LiveSessionTerminal) {
        let event = self.build_terminal_event(terminal);
        persist_live_audit_event(state, event, LIVE_AUDIT_WRITE_WAIT, "session").await;
    }

    fn build_terminal_event(self, terminal: LiveSessionTerminal) -> UsageEvent {
        let mut data = build_usage_event_data_seed(&self.plan, self.report_context.as_ref());
        data.request_type = Some("live".to_string());
        data.is_stream = Some(self.transport != LiveAuditTransport::WebRtc);
        data.status_code = Some(terminal.status_code);
        data.response_time_ms = Some(terminal.elapsed_ms);
        data.first_byte_time_ms = terminal.first_upstream_frame_ms;
        data.input_tokens = None;
        data.output_tokens = None;
        data.total_tokens = None;
        data.cache_creation_input_tokens = None;
        data.cache_creation_ephemeral_5m_input_tokens = None;
        data.cache_creation_ephemeral_1h_input_tokens = None;
        data.cache_read_input_tokens = None;
        data.cache_creation_cost_usd = None;
        data.cache_read_cost_usd = None;
        data.total_cost_usd = None;
        data.actual_total_cost_usd = None;
        if self.transport == LiveAuditTransport::WebRtc {
            clear_live_call_create_http_capture(&mut data);
        }
        if terminal.disposition != LiveSessionDisposition::Completed {
            data.error_message = Some(terminal.termination.to_string());
            data.error_category = Some(terminal.disposition.error_category().to_string());
        }
        data.request_metadata =
            attach_terminal_metadata(data.request_metadata, self.transport, &terminal);
        UsageEvent::new(
            terminal.disposition.event_type(),
            self.plan.request_id,
            data,
        )
    }
}

/// Exactly-once terminal audit for one Codex Live call-create future.
///
/// Explicit response paths call [`Self::fail`] or [`Self::complete`]. If the
/// request future is cancelled or unwinds before doing so, `Drop` emits a
/// billing-void cancellation row. Once a provider attempt exists, the guard
/// switches from the preflight seed to that attempt's routed identity without
/// changing the request ID, so a request cannot produce both rows.
pub(super) struct LiveCallCreateAuditGuard {
    inner: Box<LiveCallCreateAuditState>,
}

struct LiveCallCreateAuditState {
    state: AppState,
    decision: Option<Box<GatewayControlDecision>>,
    request_id: String,
    request_path: String,
    client_model: Option<String>,
    diagnostic: Option<LocalExecutionRuntimeMissDiagnostic>,
    attempt: Option<Box<LiveSessionAudit>>,
    started_at: Instant,
    finished: bool,
}

impl LiveCallCreateAuditGuard {
    pub(super) fn new(
        state: &AppState,
        decision: Option<&GatewayControlDecision>,
        request_id: &str,
        request_path: &str,
    ) -> Self {
        Self {
            inner: Box::new(LiveCallCreateAuditState {
                state: state.clone(),
                decision: decision.cloned().map(Box::new),
                request_id: request_id.to_string(),
                request_path: request_path.to_string(),
                client_model: None,
                diagnostic: None,
                attempt: None,
                started_at: Instant::now(),
                finished: false,
            }),
        }
    }

    /// Stores only a model that has already passed the Live identifier
    /// validator. Invalid or missing user input remains `unknown` in usage.
    pub(super) fn set_validated_client_model(&mut self, client_model: &str) {
        self.inner.client_model = Some(client_model.to_string());
    }

    pub(super) fn set_runtime_miss(
        &mut self,
        diagnostic: Option<LocalExecutionRuntimeMissDiagnostic>,
    ) {
        self.inner.diagnostic = diagnostic;
    }

    pub(super) fn bind_attempt(&mut self, attempt: &AiSyncAttempt) {
        self.inner.attempt = Some(Box::new(LiveSessionAudit::from_sync_attempt(
            attempt,
            LiveAuditTransport::WebRtc,
        )));
    }

    pub(super) fn fail(&mut self, status: http::StatusCode, termination: &'static str) {
        self.finish(LiveSessionDisposition::Failed, status.as_u16(), termination);
    }

    pub(super) fn complete(&mut self, status_code: u16, termination: &'static str) {
        self.finish(LiveSessionDisposition::Completed, status_code, termination);
    }

    fn finish(
        &mut self,
        disposition: LiveSessionDisposition,
        status_code: u16,
        termination: &'static str,
    ) {
        let inner = self.inner.as_mut();
        if std::mem::replace(&mut inner.finished, true) {
            return;
        }
        let terminal = LiveSessionTerminal {
            disposition,
            status_code,
            termination,
            elapsed_ms: inner
                .started_at
                .elapsed()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
            first_upstream_frame_ms: None,
            client_frames: 0,
            client_bytes: 0,
            upstream_frames: 0,
            upstream_bytes: 0,
        };
        let event = match inner.attempt.take() {
            Some(audit) => (*audit).build_terminal_event(terminal),
            None => build_live_preflight_event(
                inner.decision.as_deref(),
                inner.request_id.as_str(),
                inner.request_path.as_str(),
                inner.client_model.as_deref(),
                LiveAuditTransport::WebRtc,
                terminal,
                inner.diagnostic.as_ref(),
            ),
        };
        spawn_live_audit_event_detached(&inner.state, event, "call_create");
    }
}

impl Drop for LiveCallCreateAuditGuard {
    fn drop(&mut self) {
        self.finish(
            LiveSessionDisposition::Cancelled,
            499,
            "request_future_cancelled",
        );
    }
}

fn clear_live_call_create_http_capture(data: &mut UsageEventData) {
    // The call-create plan carries the exact upstream multipart/JSON bytes,
    // including SDP, instructions, and authorization headers. A lifecycle row
    // needs none of that material, so do not rely on downstream masking alone.
    data.request_headers = None;
    data.request_body = None;
    data.request_body_ref = None;
    data.request_body_state = Some(UsageBodyCaptureState::None);
    data.provider_request_headers = None;
    data.provider_request_body = None;
    data.provider_request_body_ref = None;
    data.provider_request_body_state = Some(UsageBodyCaptureState::None);
    data.response_headers = None;
    data.response_body = None;
    data.response_body_ref = None;
    data.response_body_state = Some(UsageBodyCaptureState::None);
    data.client_response_headers = None;
    data.client_response_body = None;
    data.client_response_body_ref = None;
    data.client_response_body_state = Some(UsageBodyCaptureState::None);
    if let Some(metadata) = data
        .request_metadata
        .as_mut()
        .and_then(Value::as_object_mut)
    {
        for key in [
            "request_body_ref",
            "provider_request_body_ref",
            "response_body_ref",
            "client_response_body_ref",
            "provider_request_body_base64_bytes",
            "provider_response_body_base64_bytes",
            "client_response_body_base64_bytes",
            "body_size",
        ] {
            metadata.remove(key);
        }
    }
}

async fn persist_live_audit_event(
    state: &AppState,
    event: UsageEvent,
    wait: Duration,
    audit_scope: &'static str,
) {
    if !state.usage_runtime.is_enabled() {
        return;
    }
    let request_id = event.request_id.clone();
    let usage_runtime = std::sync::Arc::clone(&state.usage_runtime);
    let usage_data = std::sync::Arc::clone(state.usage_lifecycle_data_state());
    let write_request_id = request_id.clone();
    let task = tokio::spawn(async move {
        if tokio::time::timeout(
            LIVE_AUDIT_WRITE_HARD_TIMEOUT,
            usage_runtime.record_terminal_event_direct(usage_data.as_ref(), event),
        )
        .await
        .is_err()
        {
            warn!(
                target: LIVE_AUDIT_LOG_TARGET,
                event_name = "codex_live_audit_write_timeout",
                log_type = "ops",
                request_id = write_request_id,
                audit_scope,
                wait_ms = LIVE_AUDIT_WRITE_HARD_TIMEOUT.as_millis() as u64,
                write_cancelled = true,
                "Codex Live cancelled an audit write after its hard timeout"
            );
        }
    });
    match tokio::time::timeout(wait, task).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => warn!(
            target: LIVE_AUDIT_LOG_TARGET,
            event_name = "codex_live_audit_task_failed",
            log_type = "ops",
            request_id,
            audit_scope,
            error = %error,
            "Codex Live audit task failed"
        ),
        Err(_) => warn!(
            target: LIVE_AUDIT_LOG_TARGET,
            event_name = "codex_live_audit_write_slow",
            log_type = "ops",
            request_id,
            audit_scope,
            wait_ms = wait.as_millis() as u64,
            hard_timeout_ms = LIVE_AUDIT_WRITE_HARD_TIMEOUT.as_millis() as u64,
            write_detached = true,
            "Codex Live stopped waiting for a slow audit write"
        ),
    }
}

fn spawn_live_audit_event_detached(state: &AppState, event: UsageEvent, audit_scope: &'static str) {
    if !state.usage_runtime.is_enabled() {
        return;
    }
    let request_id = event.request_id.clone();
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        warn!(
            target: LIVE_AUDIT_LOG_TARGET,
            event_name = "codex_live_audit_runtime_unavailable",
            log_type = "ops",
            request_id,
            audit_scope,
            write_detached = true,
            "Codex Live could not spawn a detached audit write without a Tokio runtime"
        );
        return;
    };
    let usage_runtime = std::sync::Arc::clone(&state.usage_runtime);
    let usage_data = std::sync::Arc::clone(state.usage_lifecycle_data_state());
    runtime.spawn(async move {
        if tokio::time::timeout(
            LIVE_AUDIT_WRITE_HARD_TIMEOUT,
            usage_runtime.record_terminal_event_direct(usage_data.as_ref(), event),
        )
        .await
        .is_err()
        {
            warn!(
                target: LIVE_AUDIT_LOG_TARGET,
                event_name = "codex_live_audit_write_timeout",
                log_type = "ops",
                request_id,
                audit_scope,
                write_detached = true,
                wait_ms = LIVE_AUDIT_WRITE_HARD_TIMEOUT.as_millis() as u64,
                write_cancelled = true,
                "Codex Live cancelled a detached audit write after its hard timeout"
            );
        }
    });
}

fn attach_terminal_metadata(
    metadata: Option<Value>,
    transport: LiveAuditTransport,
    terminal: &LiveSessionTerminal,
) -> Option<Value> {
    let mut object = match metadata {
        Some(Value::Object(object)) => object,
        _ => Map::new(),
    };
    object.insert(USAGE_AVAILABLE_METADATA_KEY.to_string(), Value::Bool(false));
    object.insert(
        USAGE_PRICING_AVAILABLE_METADATA_KEY.to_string(),
        Value::Bool(false),
    );
    object.insert(
        WEBSOCKET_MODE_METADATA_KEY.to_string(),
        Value::Bool(transport.websocket_transport().is_some()),
    );
    if let Some(websocket_transport) = transport.websocket_transport() {
        object.insert(
            WEBSOCKET_TRANSPORT_METADATA_KEY.to_string(),
            Value::String(websocket_transport.to_string()),
        );
    }
    object.insert(
        LIVE_SESSION_METADATA_KEY.to_string(),
        json!({
            "schema_version": LIVE_AUDIT_SCHEMA_VERSION,
            "transport": transport.transport(),
            "mode": transport.mode(),
            "state": terminal.disposition.state(),
            "termination": terminal.termination,
            "elapsed_ms": terminal.elapsed_ms,
            "client_frames": terminal.client_frames,
            "client_bytes": terminal.client_bytes,
            "upstream_frames": terminal.upstream_frames,
            "upstream_bytes": terminal.upstream_bytes,
            "first_upstream_frame_ms": terminal.first_upstream_frame_ms,
            "usage_state": "unavailable",
        }),
    );
    Some(Value::Object(object))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LiveSessionDisposition {
    Completed,
    Failed,
    Cancelled,
}

impl LiveSessionDisposition {
    const fn event_type(self) -> UsageEventType {
        match self {
            Self::Completed => UsageEventType::Completed,
            Self::Failed => UsageEventType::Failed,
            Self::Cancelled => UsageEventType::Cancelled,
        }
    }

    const fn state(self) -> &'static str {
        match self {
            Self::Completed => "closed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    const fn error_category(self) -> &'static str {
        match self {
            Self::Completed => "none",
            Self::Failed => "transport_error",
            Self::Cancelled => "client_cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct LiveSessionTerminal {
    pub(super) disposition: LiveSessionDisposition,
    pub(super) status_code: u16,
    pub(super) termination: &'static str,
    pub(super) elapsed_ms: u64,
    pub(super) first_upstream_frame_ms: Option<u64>,
    pub(super) client_frames: u64,
    pub(super) client_bytes: u64,
    pub(super) upstream_frames: u64,
    pub(super) upstream_bytes: u64,
}

impl LiveSessionTerminal {
    #[cfg(test)]
    pub(super) const fn completed(
        status_code: u16,
        termination: &'static str,
        elapsed_ms: u64,
    ) -> Self {
        Self {
            disposition: LiveSessionDisposition::Completed,
            status_code,
            termination,
            elapsed_ms,
            first_upstream_frame_ms: None,
            client_frames: 0,
            client_bytes: 0,
            upstream_frames: 0,
            upstream_bytes: 0,
        }
    }

    pub(super) const fn failure(
        status_code: u16,
        termination: &'static str,
        elapsed_ms: u64,
    ) -> Self {
        Self {
            disposition: LiveSessionDisposition::Failed,
            status_code,
            termination,
            elapsed_ms,
            first_upstream_frame_ms: None,
            client_frames: 0,
            client_bytes: 0,
            upstream_frames: 0,
            upstream_bytes: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use aether_contracts::{ExecutionTimeouts, RequestBody};
    use aether_data::repository::usage::InMemoryUsageReadRepository;
    use aether_data_contracts::repository::usage::{UsageAuditListQuery, UsageReadRepository};
    use aether_usage_runtime::build_upsert_usage_record_from_event;

    use crate::control::{GatewayControlAuthContext, GatewayControlDecision};
    use crate::state::LocalExecutionRuntimeMissDiagnostic;

    use super::*;

    fn sample_attempt() -> AiStreamAttempt {
        AiStreamAttempt {
            plan: ExecutionPlan {
                request_id: "live-request".to_string(),
                candidate_id: Some("candidate-live".to_string()),
                provider_name: Some("Codex".to_string()),
                provider_id: "provider-live".to_string(),
                endpoint_id: "endpoint-live".to_string(),
                key_id: "key-live".to_string(),
                method: "GET".to_string(),
                url: "wss://example.test/v1/live".to_string(),
                headers: BTreeMap::new(),
                content_type: None,
                content_encoding: None,
                body: RequestBody {
                    json_body: None,
                    body_bytes_b64: None,
                    body_ref: None,
                },
                stream: true,
                client_api_format: "codex:live".to_string(),
                provider_api_format: "codex:live".to_string(),
                model_name: Some("gpt-live".to_string()),
                proxy: None,
                transport_profile: None,
                timeouts: Some(ExecutionTimeouts::default()),
            },
            report_kind: Some("openai_responses_stream".to_string()),
            report_context: Some(json!({
                "user_id": "user-live",
                "api_key_id": "gateway-key-live",
                "trace_id": "trace-live"
            })),
        }
    }

    #[test]
    fn direct_terminal_audit_is_opaque_unmetered_and_void_eligible() {
        let audit =
            LiveSessionAudit::from_attempt(&sample_attempt(), LiveAuditTransport::DirectWebSocket);
        let event = audit.build_terminal_event(LiveSessionTerminal {
            disposition: LiveSessionDisposition::Completed,
            status_code: 200,
            termination: "client_close_frame",
            elapsed_ms: 1234,
            first_upstream_frame_ms: Some(42),
            client_frames: 3,
            client_bytes: 128,
            upstream_frames: 5,
            upstream_bytes: 512,
        });

        assert_eq!(event.event_type, UsageEventType::Completed);
        assert_eq!(event.data.input_tokens, None);
        assert_eq!(event.data.total_cost_usd, None);
        let metadata = event.data.request_metadata.expect("metadata");
        assert_eq!(metadata[USAGE_AVAILABLE_METADATA_KEY], false);
        assert_eq!(metadata[USAGE_PRICING_AVAILABLE_METADATA_KEY], false);
        assert_eq!(metadata[WEBSOCKET_MODE_METADATA_KEY], true);
        assert_eq!(
            metadata[WEBSOCKET_TRANSPORT_METADATA_KEY],
            "codex_live_direct"
        );
        assert_eq!(metadata[LIVE_SESSION_METADATA_KEY]["client_frames"], 3);
        assert_eq!(
            metadata[LIVE_SESSION_METADATA_KEY]["usage_state"],
            "unavailable"
        );
    }

    #[test]
    fn call_create_is_webrtc_not_websocket() {
        let mut context = Some(json!({"trace_id": "trace-live"}));
        mark_live_call_create_report_context(&mut context);
        let context = context.expect("context");

        assert_eq!(context[USAGE_AVAILABLE_METADATA_KEY], false);
        assert_eq!(context[USAGE_PRICING_AVAILABLE_METADATA_KEY], false);
        assert_eq!(context[WEBSOCKET_MODE_METADATA_KEY], false);
        assert!(context.get(WEBSOCKET_TRANSPORT_METADATA_KEY).is_none());
        assert_eq!(context[LIVE_SESSION_METADATA_KEY]["transport"], "webrtc");

        let mut sensitive_attempt = sample_attempt();
        sensitive_attempt.plan.headers.insert(
            "authorization".to_string(),
            "Bearer upstream-token-sentinel".to_string(),
        );
        sensitive_attempt.plan.body.json_body = Some(json!({
            "sdp": "sdp-secret-sentinel",
            "session": {"instructions": "instruction-secret-sentinel"}
        }));
        sensitive_attempt.report_context = Some(json!({
            "trace_id": "trace-live",
            "user_id": "user-live",
            "api_key_id": "gateway-key-live",
            "original_headers": {
                "authorization": "Bearer downstream-token-sentinel"
            },
            "original_request_body": {
                "sdp": "sdp-secret-sentinel",
                "session": {"instructions": "instruction-secret-sentinel"}
            },
            "provider_request_headers": {
                "authorization": "Bearer upstream-token-sentinel"
            },
            "provider_request_body": {
                "sdp": "sdp-secret-sentinel",
                "session": {"instructions": "instruction-secret-sentinel"}
            }
        }));
        let event = LiveSessionAudit::from_attempt(&sensitive_attempt, LiveAuditTransport::WebRtc)
            .build_terminal_event(LiveSessionTerminal::completed(201, "call_created", 18));
        let serialized_event = serde_json::to_string(&event).expect("event should serialize");
        for sentinel in [
            "sdp-secret-sentinel",
            "instruction-secret-sentinel",
            "downstream-token-sentinel",
            "upstream-token-sentinel",
        ] {
            assert!(!serialized_event.contains(sentinel));
        }
        let record = build_upsert_usage_record_from_event(&event)
            .expect("Live call-create usage record should build");
        let serialized_record =
            serde_json::to_string(&record).expect("usage record should serialize");
        for sentinel in [
            "sdp-secret-sentinel",
            "instruction-secret-sentinel",
            "downstream-token-sentinel",
            "upstream-token-sentinel",
        ] {
            assert!(!serialized_record.contains(sentinel));
        }
        assert_eq!(record.status, "completed");
        assert_eq!(record.billing_status, "void");
        assert_eq!(record.status_code, Some(201));
        assert_eq!(record.request_type.as_deref(), Some("live"));
        assert_eq!(record.api_format.as_deref(), Some("codex:live"));
        assert_eq!(record.is_stream, Some(false));
        assert_eq!(record.total_tokens, None);
        assert_eq!(record.total_cost_usd, None);
        assert!(record.request_headers.is_none());
        assert!(record.request_body.is_none());
        assert!(record.provider_request_headers.is_none());
        assert!(record.provider_request_body.is_none());
        let metadata = record.request_metadata.expect("call-create metadata");
        assert_eq!(metadata[WEBSOCKET_MODE_METADATA_KEY], false);
        assert!(metadata.get(WEBSOCKET_TRANSPORT_METADATA_KEY).is_none());
        assert_eq!(metadata[USAGE_AVAILABLE_METADATA_KEY], false);
        assert_eq!(metadata[USAGE_PRICING_AVAILABLE_METADATA_KEY], false);
        assert_eq!(metadata[LIVE_SESSION_METADATA_KEY]["mode"], "call_create");
        assert_eq!(
            metadata[LIVE_SESSION_METADATA_KEY]["termination"],
            "call_created"
        );
    }

    #[test]
    fn call_create_capture_scrubber_removes_every_http_surface() {
        let secret = json!({"secret": "live-http-capture-sentinel"});
        let secret_ref = Some("usage://live-http-capture-sentinel".to_string());
        let mut data = UsageEventData {
            provider_name: "Codex".to_string(),
            model: "gpt-live".to_string(),
            request_type: Some("live".to_string()),
            api_format: Some("codex:live".to_string()),
            status_code: Some(201),
            request_headers: Some(secret.clone()),
            request_body: Some(secret.clone()),
            request_body_ref: secret_ref.clone(),
            request_body_state: Some(UsageBodyCaptureState::Inline),
            provider_request_headers: Some(secret.clone()),
            provider_request_body: Some(secret.clone()),
            provider_request_body_ref: secret_ref.clone(),
            provider_request_body_state: Some(UsageBodyCaptureState::Reference),
            response_headers: Some(secret.clone()),
            response_body: Some(secret.clone()),
            response_body_ref: secret_ref.clone(),
            response_body_state: Some(UsageBodyCaptureState::Truncated),
            client_response_headers: Some(secret.clone()),
            client_response_body: Some(secret),
            client_response_body_ref: secret_ref,
            client_response_body_state: Some(UsageBodyCaptureState::Unavailable),
            request_metadata: Some(json!({
                "request_body_ref": "usage://live-http-capture-sentinel/request",
                "provider_request_body_ref": "usage://live-http-capture-sentinel/provider-request",
                "response_body_ref": "usage://live-http-capture-sentinel/response",
                "client_response_body_ref": "usage://live-http-capture-sentinel/client-response",
                "provider_request_body_base64_bytes": 101,
                "provider_response_body_base64_bytes": 202,
                "client_response_body_base64_bytes": 303,
                "body_size": 404,
                "retained": true,
            })),
            ..UsageEventData::default()
        };

        clear_live_call_create_http_capture(&mut data);

        for value in [
            data.request_headers.as_ref(),
            data.request_body.as_ref(),
            data.provider_request_headers.as_ref(),
            data.provider_request_body.as_ref(),
            data.response_headers.as_ref(),
            data.response_body.as_ref(),
            data.client_response_headers.as_ref(),
            data.client_response_body.as_ref(),
        ] {
            assert!(value.is_none());
        }
        assert!(data.request_body_ref.is_none());
        assert!(data.provider_request_body_ref.is_none());
        assert!(data.response_body_ref.is_none());
        assert!(data.client_response_body_ref.is_none());
        assert_eq!(data.request_body_state, Some(UsageBodyCaptureState::None));
        assert_eq!(
            data.provider_request_body_state,
            Some(UsageBodyCaptureState::None)
        );
        assert_eq!(data.response_body_state, Some(UsageBodyCaptureState::None));
        assert_eq!(
            data.client_response_body_state,
            Some(UsageBodyCaptureState::None)
        );
        let metadata = data
            .request_metadata
            .as_ref()
            .and_then(Value::as_object)
            .expect("capture metadata should remain an object");
        for key in [
            "request_body_ref",
            "provider_request_body_ref",
            "response_body_ref",
            "client_response_body_ref",
            "provider_request_body_base64_bytes",
            "provider_response_body_base64_bytes",
            "client_response_body_base64_bytes",
            "body_size",
        ] {
            assert!(metadata.get(key).is_none());
        }
        assert_eq!(metadata.get("retained"), Some(&Value::Bool(true)));

        let event = UsageEvent::new(UsageEventType::Completed, "live-capture", data);
        let serialized_event = serde_json::to_string(&event).expect("event should serialize");
        assert!(!serialized_event.contains("live-http-capture-sentinel"));
        let record = build_upsert_usage_record_from_event(&event)
            .expect("scrubbed Live call-create record should build");
        let serialized_record = serde_json::to_string(&record).expect("record should serialize");
        assert!(!serialized_record.contains("live-http-capture-sentinel"));
        assert!(record.request_headers.is_none());
        assert!(record.request_body.is_none());
        assert!(record.request_body_ref.is_none());
        assert!(record.provider_request_headers.is_none());
        assert!(record.provider_request_body.is_none());
        assert!(record.provider_request_body_ref.is_none());
        assert!(record.response_headers.is_none());
        assert!(record.response_body.is_none());
        assert!(record.response_body_ref.is_none());
        assert!(record.client_response_headers.is_none());
        assert!(record.client_response_body.is_none());
        assert!(record.client_response_body_ref.is_none());
    }

    #[test]
    fn candidate_unavailable_call_create_is_failed_void_unmetered_live_http() {
        let mut decision = GatewayControlDecision::synthetic(
            "/v1/live",
            Some("ai_public".to_string()),
            Some("codex".to_string()),
            Some("live".to_string()),
            Some("codex:live".to_string()),
        );
        decision.auth_context = Some(GatewayControlAuthContext {
            user_id: "user-live-preflight".to_string(),
            api_key_id: "key-live-preflight".to_string(),
            username: Some("live-user".to_string()),
            api_key_name: Some("live-key".to_string()),
            balance_remaining: None,
            access_allowed: true,
            user_rate_limit: None,
            sell_rate_multiplier: 1.0,
            api_key_rate_limit: None,
            api_key_is_standalone: true,
            admin_bypass_limits: false,
            local_rejection: None,
            allowed_models: None,
            ip_rules: None,
            billing_group_id: None,
            billing_group_name: None,
        });
        let diagnostic = LocalExecutionRuntimeMissDiagnostic {
            reason: "candidate_list_empty".to_string(),
            route_family: Some("codex".to_string()),
            route_kind: Some("live".to_string()),
            requested_model: Some("gpt-live-unmapped".to_string()),
            ..LocalExecutionRuntimeMissDiagnostic::default()
        };

        let event = build_live_preflight_event(
            Some(&decision),
            "trace-live-preflight",
            "/v1/realtime/calls",
            Some("gpt-live-unmapped"),
            LiveAuditTransport::WebRtc,
            LiveSessionTerminal::failure(503, "candidate_unavailable", 37),
            Some(&diagnostic),
        );
        assert_eq!(event.event_type, UsageEventType::Failed);
        assert_eq!(event.data.request_type.as_deref(), Some("live"));
        assert_eq!(event.data.api_format.as_deref(), Some("codex:live"));
        assert_eq!(event.data.model, "gpt-live-unmapped");
        assert_eq!(event.data.status_code, Some(503));
        assert_eq!(event.data.is_stream, Some(false));
        assert!(event.data.provider_id.is_none());
        assert!(event.data.request_headers.is_none());
        assert!(event.data.request_body.is_none());
        assert!(event.data.provider_request_headers.is_none());
        assert!(event.data.provider_request_body.is_none());

        let record = build_upsert_usage_record_from_event(&event)
            .expect("Live preflight usage record should build");
        assert_eq!(record.status, "failed");
        assert_eq!(record.billing_status, "void");
        assert_eq!(record.status_code, Some(503));
        assert_eq!(record.request_type.as_deref(), Some("live"));
        assert_eq!(record.api_format.as_deref(), Some("codex:live"));
        assert_eq!(record.total_tokens, None);
        assert_eq!(record.total_cost_usd, None);
        let metadata = record.request_metadata.expect("Live metadata");
        assert_eq!(metadata[USAGE_AVAILABLE_METADATA_KEY], false);
        assert_eq!(metadata[USAGE_PRICING_AVAILABLE_METADATA_KEY], false);
        assert_eq!(metadata[WEBSOCKET_MODE_METADATA_KEY], false);
        assert!(metadata.get(WEBSOCKET_TRANSPORT_METADATA_KEY).is_none());
        assert_eq!(metadata["request_path"], "/v1/realtime/calls");
        assert_eq!(
            metadata[LIVE_SESSION_METADATA_KEY]["termination"],
            "candidate_unavailable"
        );
        assert_eq!(metadata[LIVE_SESSION_METADATA_KEY]["state"], "failed");
    }

    #[tokio::test]
    async fn call_create_guard_records_cancellation_once_and_disarms_after_explicit_finish() {
        let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
        let state = AppState::new()
            .expect("gateway state should build")
            .with_usage_data_repository_for_tests(Arc::clone(&usage_repository))
            .with_usage_runtime_for_tests(crate::usage::UsageRuntimeConfig {
                enabled: true,
                ..crate::usage::UsageRuntimeConfig::default()
            });
        let decision = GatewayControlDecision::synthetic(
            "/v1/live",
            Some("ai_public".to_string()),
            Some("codex".to_string()),
            Some("live".to_string()),
            Some("codex:live".to_string()),
        );

        {
            let mut guard = LiveCallCreateAuditGuard::new(
                &state,
                Some(&decision),
                "live-call-create-cancelled",
                "/v1/live",
            );
            guard.set_validated_client_model("gpt-live-cancelled");
        }
        {
            let mut guard = LiveCallCreateAuditGuard::new(
                &state,
                Some(&decision),
                "live-call-create-explicit",
                "/v1/live",
            );
            guard.set_validated_client_model("gpt-live-explicit");
            guard.fail(http::StatusCode::BAD_REQUEST, "explicit_failure");
        }

        let (cancelled, explicit) = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let cancelled = usage_repository
                    .find_by_request_id("live-call-create-cancelled")
                    .await
                    .expect("cancelled Live usage read should succeed");
                let explicit = usage_repository
                    .find_by_request_id("live-call-create-explicit")
                    .await
                    .expect("explicit Live usage read should succeed");
                if let (Some(cancelled), Some(explicit)) = (cancelled, explicit) {
                    break (cancelled, explicit);
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("both detached Live audits should persist before timeout");

        assert_eq!(cancelled.status, "cancelled");
        assert_eq!(cancelled.billing_status, "void");
        assert_eq!(cancelled.status_code, Some(499));
        assert_eq!(cancelled.request_type.as_deref(), Some("live"));
        assert_eq!(cancelled.api_format.as_deref(), Some("codex:live"));
        assert_eq!(cancelled.model, "gpt-live-cancelled");
        assert_eq!(
            cancelled.request_metadata.as_ref().and_then(|metadata| {
                metadata[LIVE_SESSION_METADATA_KEY]["termination"].as_str()
            }),
            Some("request_future_cancelled")
        );

        // Dropping the explicitly-finished guard must not enqueue a second
        // cancellation that races with or overwrites its intended terminal.
        tokio::time::sleep(Duration::from_millis(25)).await;
        let explicit = usage_repository
            .find_by_request_id("live-call-create-explicit")
            .await
            .expect("explicit Live usage read should succeed")
            .expect("explicit Live usage should remain present");
        assert_eq!(explicit.status, "failed");
        assert_eq!(explicit.status_code, Some(400));
        assert_eq!(
            explicit.request_metadata.as_ref().and_then(|metadata| {
                metadata[LIVE_SESSION_METADATA_KEY]["termination"].as_str()
            }),
            Some("explicit_failure")
        );

        let rows = usage_repository
            .list_usage_audits(&UsageAuditListQuery {
                limit: Some(10),
                ..UsageAuditListQuery::default()
            })
            .await
            .expect("Live usage list should succeed");
        assert_eq!(rows.len(), 2);
    }

    #[tokio::test]
    async fn websocket_preflight_failure_is_persisted_as_one_void_websocket_row() {
        let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
        let state = AppState::new()
            .expect("gateway state should build")
            .with_usage_data_repository_for_tests(Arc::clone(&usage_repository))
            .with_usage_runtime_for_tests(crate::usage::UsageRuntimeConfig {
                enabled: true,
                ..crate::usage::UsageRuntimeConfig::default()
            });
        let mut decision = GatewayControlDecision::synthetic(
            "/v1/realtime",
            Some("ai_public".to_string()),
            Some("codex".to_string()),
            Some("live".to_string()),
            Some("codex:live".to_string()),
        );
        decision.auth_context = Some(GatewayControlAuthContext {
            user_id: "user-live-ws-preflight".to_string(),
            api_key_id: "key-live-ws-preflight".to_string(),
            username: Some("live-ws-user".to_string()),
            api_key_name: Some("live-ws-key".to_string()),
            balance_remaining: None,
            access_allowed: true,
            user_rate_limit: None,
            sell_rate_multiplier: 1.0,
            api_key_rate_limit: None,
            api_key_is_standalone: true,
            admin_bypass_limits: false,
            local_rejection: None,
            allowed_models: None,
            ip_rules: None,
            billing_group_id: None,
            billing_group_name: None,
        });

        record_live_websocket_preflight_failure(
            &state,
            &decision,
            "live-ws-preflight-failed",
            "/v1/realtime",
            Some("model=gpt-realtime-1.5"),
            http::StatusCode::SERVICE_UNAVAILABLE,
            "candidate_unavailable",
        );

        let record = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(record) = usage_repository
                    .find_by_request_id("live-ws-preflight-failed")
                    .await
                    .expect("Live WebSocket preflight usage read should succeed")
                {
                    break record;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("Live WebSocket preflight audit should persist before timeout");

        assert_eq!(record.status, "failed");
        assert_eq!(record.billing_status, "void");
        assert_eq!(record.status_code, Some(503));
        assert_eq!(record.request_type.as_deref(), Some("live"));
        assert_eq!(record.api_format.as_deref(), Some("codex:live"));
        assert_eq!(record.model, "gpt-realtime-1.5");
        assert!(record.is_stream);
        assert!(record.is_websocket());
        assert_eq!(record.websocket_transport(), Some("codex_live_direct"));
        assert!(!record.usage_available());
        assert!(!record.usage_pricing_available());
        assert_eq!(record.input_tokens, 0);
        assert_eq!(record.output_tokens, 0);
        assert_eq!(record.total_tokens, 0);
        assert_eq!(record.total_cost_usd, 0.0);
        assert_eq!(record.actual_total_cost_usd, 0.0);
        assert_eq!(
            record.execution_path.as_deref(),
            Some("codex_live_websocket_preflight")
        );

        let rows = usage_repository
            .list_usage_audits(&UsageAuditListQuery {
                limit: Some(10),
                ..UsageAuditListQuery::default()
            })
            .await
            .expect("Live WebSocket usage list should succeed");
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn call_create_guard_drop_without_tokio_runtime_does_not_panic() {
        let state = AppState::new()
            .expect("gateway state should build")
            .with_usage_runtime_for_tests(crate::usage::UsageRuntimeConfig {
                enabled: true,
                ..crate::usage::UsageRuntimeConfig::default()
            });
        let guard = LiveCallCreateAuditGuard::new(
            &state,
            None,
            "live-call-create-without-runtime",
            "/v1/live",
        );
        drop(guard);
    }
}
