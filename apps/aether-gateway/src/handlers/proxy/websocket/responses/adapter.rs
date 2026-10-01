//! Provider-specific hooks for the standard Responses WebSocket session.

use async_trait::async_trait;
use serde_json::Value;
use std::borrow::Cow;

use super::adapters::CODEX_RESPONSES_WEBSOCKET_ADAPTER;
use crate::ai_serving::AiExecutionDecision;
use crate::handlers::proxy::websocket::transport::UpstreamWebSocketErrorCodes;
use crate::orchestration::ResponsesWebSocketAdapter;
use crate::AppState;

#[derive(Debug, Clone, Copy)]
pub(super) struct ResponsesWebSocketDrainDirective {
    pub(super) error_code: &'static str,
    /// The terminal upstream event may be replayed only when the session has
    /// not exposed any standard Responses event to the client.
    pub(super) retry_current_turn: bool,
    /// When present, the exhausted provider key remains excluded from later
    /// turns on this client socket until the upstream's reported reset time.
    pub(super) retry_exclusion_until_unix_secs: Option<u64>,
}

/// Provider-specific observation produced while relaying an upstream frame.
/// The session can make the retry/drain decision synchronously, while the
/// optional persistence sink runs outside the frame-forwarding path.
#[derive(Debug, Clone)]
pub(super) struct ResponsesWebSocketAdapterObservation {
    pub(super) drain: Option<ResponsesWebSocketDrainDirective>,
    pub(super) quota_metadata: Option<Value>,
}

/// Provider identity used by the shared session's temporary exclusion table.
/// The session does not need to know how a provider derives its account id.
#[derive(Debug, Clone, Default)]
pub(super) struct ResponsesWebSocketExclusionIdentity {
    pub(super) account_id: Option<String>,
}

/// Whether receiving an upstream event still leaves the active client turn
/// safe to replay on a freshly bound upstream.  The shared session keeps the
/// conservative default; provider adapters may explicitly whitelist their
/// documented, pre-response advisory events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResponsesWebSocketRebindSafety {
    Safe,
    Unsafe { reason: &'static str },
}

/// How an upstream text frame crosses the public Responses WebSocket boundary.
///
/// The normal path is deliberately byte-opaque: callers forward the parsed
/// frame's original text without rebuilding it from a gateway-owned schema.
/// Codex is the only adapter that may peel its documented private batch
/// envelope.  Even then, the retained events are borrowed whole so unknown
/// `response.*` event types and unknown fields survive unchanged.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum ResponsesWebSocketRelayDirective<'a> {
    /// Forward the provider frame's original text exactly as received.
    ForwardOriginal,
    /// The provider frame was a private batch envelope. Forward each retained
    /// event in document order by serializing the complete borrowed value.
    ForwardEvents(Vec<Cow<'a, Value>>),
    /// The entire frame was an explicitly recognized provider-private
    /// envelope and therefore has no public event to relay.
    SuppressProviderPrivate,
}

/// Boundary between the standard Responses protocol engine and provider
/// behavior. Adapters receive already-planned provider requests; they never
/// own public WebSocket parsing, turn accounting, or model scheduling.
#[async_trait]
pub(super) trait ResponsesWebSocketProtocolAdapter: Send + Sync {
    fn kind(&self) -> ResponsesWebSocketAdapter;

    fn upstream_errors(&self) -> UpstreamWebSocketErrorCodes;

    /// Adds provider-specific metadata to an otherwise standard Responses
    /// stream report. The event payload is never rewritten for the client.
    fn decorate_turn_report_context(&self, report_context: &mut Option<Value>, event: &Value);

    /// Whether this adapter needs the shared session to parse each upstream
    /// text event before normal turn accounting runs.
    fn observes_upstream_events(&self) -> bool;

    /// Classifies whether a received upstream event can be followed by a
    /// transparent quota-driven rebind.  An adapter must return `Safe` only
    /// for events that neither create public Responses state nor make a replay
    /// observably ambiguous to the client.
    fn rebind_safety_for_upstream_event(&self, event: &Value) -> ResponsesWebSocketRebindSafety;

    /// 公开事件保持完整；提供商私有元数据仅投影到客户端使用的公开字段，
    /// 不公开账户信息。
    fn relay_directive_for_upstream_event<'a>(
        &self,
        _event: &'a Value,
    ) -> ResponsesWebSocketRelayDirective<'a> {
        ResponsesWebSocketRelayDirective::ForwardOriginal
    }

    /// Lets an adapter classify provider-only events. Returning a directive
    /// asks the shared session to drain after the active standard response.
    fn observe_upstream_event(&self, event: &Value)
        -> Option<ResponsesWebSocketAdapterObservation>;

    fn exhaustion_exclusion_identity(
        &self,
        _decision: &AiExecutionDecision,
    ) -> Option<ResponsesWebSocketExclusionIdentity> {
        None
    }

    /// Persists an adapter observation outside the frame-forwarding path.
    async fn persist_upstream_observation(
        &self,
        state: &AppState,
        trace_id: &str,
        report_context: Option<&Value>,
        observation: ResponsesWebSocketAdapterObservation,
    );
}

pub(super) fn resolve_responses_websocket_adapter(
    kind: ResponsesWebSocketAdapter,
) -> &'static dyn ResponsesWebSocketProtocolAdapter {
    match kind {
        ResponsesWebSocketAdapter::Standard => &STANDARD_RESPONSES_WEBSOCKET_ADAPTER,
        ResponsesWebSocketAdapter::Codex => &CODEX_RESPONSES_WEBSOCKET_ADAPTER,
    }
}

struct StandardResponsesWebSocketAdapter;

const STANDARD_UPSTREAM_WEBSOCKET_ERRORS: UpstreamWebSocketErrorCodes =
    UpstreamWebSocketErrorCodes {
        upstream_url_missing: "responses_upstream_url_missing",
        upstream_url_invalid: "responses_upstream_url_invalid",
        frontdoor_self_loop: "responses_websocket_frontdoor_self_loop",
        headers_invalid: "responses_websocket_headers_invalid",
        client_build_failed: "responses_websocket_client_build_failed",
        proxy_invalid: "responses_websocket_proxy_invalid",
        tunnel_proxy_unsupported: "responses_websocket_tunnel_proxy_unsupported",
        handshake_failed: "responses_websocket_handshake_failed",
        upgrade_rejected: "responses_websocket_upgrade_rejected",
        upgrade_failed: "responses_websocket_upgrade_failed",
    };

static STANDARD_RESPONSES_WEBSOCKET_ADAPTER: StandardResponsesWebSocketAdapter =
    StandardResponsesWebSocketAdapter;

#[async_trait]
impl ResponsesWebSocketProtocolAdapter for StandardResponsesWebSocketAdapter {
    fn kind(&self) -> ResponsesWebSocketAdapter {
        ResponsesWebSocketAdapter::Standard
    }

    fn upstream_errors(&self) -> UpstreamWebSocketErrorCodes {
        STANDARD_UPSTREAM_WEBSOCKET_ERRORS
    }

    fn decorate_turn_report_context(&self, _report_context: &mut Option<Value>, _event: &Value) {}

    fn observes_upstream_events(&self) -> bool {
        false
    }

    fn rebind_safety_for_upstream_event(&self, event: &Value) -> ResponsesWebSocketRebindSafety {
        let reason = if is_standard_responses_event(event) {
            "standard_response_event"
        } else {
            "unrecognized_upstream_event"
        };
        ResponsesWebSocketRebindSafety::Unsafe { reason }
    }

    fn observe_upstream_event(
        &self,
        _event: &Value,
    ) -> Option<ResponsesWebSocketAdapterObservation> {
        None
    }

    async fn persist_upstream_observation(
        &self,
        _state: &AppState,
        _trace_id: &str,
        _report_context: Option<&Value>,
        _observation: ResponsesWebSocketAdapterObservation,
    ) {
    }
}

pub(super) fn is_standard_responses_event(event: &Value) -> bool {
    event
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|event_type| event_type.starts_with("response."))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        resolve_responses_websocket_adapter, ResponsesWebSocketProtocolAdapter,
        ResponsesWebSocketRelayDirective,
    };
    use crate::orchestration::ResponsesWebSocketAdapter;

    #[test]
    fn standard_adapter_has_no_codex_extensions() {
        let adapter = resolve_responses_websocket_adapter(ResponsesWebSocketAdapter::Standard);

        assert_eq!(adapter.kind(), ResponsesWebSocketAdapter::Standard);
        assert!(!adapter.observes_upstream_events());
        assert_eq!(
            adapter.upstream_errors().handshake_failed,
            "responses_websocket_handshake_failed"
        );
        assert_eq!(
            adapter.upstream_errors().frontdoor_self_loop,
            "responses_websocket_frontdoor_self_loop"
        );
    }

    #[test]
    fn standard_adapter_always_forwards_future_events_opaquely() {
        let adapter = resolve_responses_websocket_adapter(ResponsesWebSocketAdapter::Standard);
        let event = json!({
            "type": "response.future_capability.delta",
            "delta": {"future_shape": [1, {"nested": true}]},
            "unknown_top_level": {"must": "survive"},
        });

        assert_eq!(
            adapter.relay_directive_for_upstream_event(&event),
            ResponsesWebSocketRelayDirective::ForwardOriginal
        );
    }
}
