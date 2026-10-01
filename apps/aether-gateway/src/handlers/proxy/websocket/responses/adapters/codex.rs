//! Codex-specific extensions for the standard Responses WebSocket session.

use async_trait::async_trait;
use serde_json::{Map, Value};
use std::borrow::Cow;

use super::super::adapter::{
    is_standard_responses_event, ResponsesWebSocketAdapterObservation,
    ResponsesWebSocketDrainDirective, ResponsesWebSocketExclusionIdentity,
    ResponsesWebSocketProtocolAdapter, ResponsesWebSocketRebindSafety,
    ResponsesWebSocketRelayDirective,
};
use crate::ai_serving::AiExecutionDecision;
use crate::clock::current_unix_secs;
use crate::handlers::proxy::websocket::transport::UpstreamWebSocketErrorCodes;
use crate::orchestration::{
    codex_account_id_from_headers, codex_model_quota_exhaustion_reset_at,
    codex_quota_exhaustion_reset_at, sync_codex_websocket_quota_metadata,
    ResponsesWebSocketAdapter,
};
use crate::AppState;

const CODEX_WEBSOCKET_LOG_TARGET: &str = "aether_gateway::handlers::proxy::codex_ws";
const CODEX_WEBSOCKET_RATE_LIMITS_REPORT_CONTEXT_FIELD: &str = "codex_websocket_rate_limits";

const CODEX_UPSTREAM_WEBSOCKET_ERRORS: UpstreamWebSocketErrorCodes = UpstreamWebSocketErrorCodes {
    upstream_url_missing: "codex_upstream_url_missing",
    upstream_url_invalid: "codex_upstream_url_invalid",
    frontdoor_self_loop: "codex_websocket_frontdoor_self_loop",
    headers_invalid: "codex_websocket_headers_invalid",
    client_build_failed: "codex_websocket_client_build_failed",
    proxy_invalid: "codex_websocket_proxy_invalid",
    tunnel_proxy_unsupported: "codex_websocket_tunnel_proxy_unsupported",
    handshake_failed: "codex_websocket_handshake_failed",
    upgrade_rejected: "codex_websocket_upgrade_rejected",
    upgrade_failed: "codex_websocket_upgrade_failed",
};

pub(crate) static CODEX_RESPONSES_WEBSOCKET_ADAPTER: CodexResponsesWebSocketAdapter =
    CodexResponsesWebSocketAdapter;

pub(crate) struct CodexResponsesWebSocketAdapter;

#[async_trait]
impl ResponsesWebSocketProtocolAdapter for CodexResponsesWebSocketAdapter {
    fn kind(&self) -> ResponsesWebSocketAdapter {
        ResponsesWebSocketAdapter::Codex
    }

    fn upstream_errors(&self) -> UpstreamWebSocketErrorCodes {
        CODEX_UPSTREAM_WEBSOCKET_ERRORS
    }

    fn decorate_turn_report_context(&self, report_context: &mut Option<Value>, event: &Value) {
        let Some(rate_limits) = parse_codex_rate_limits(event) else {
            return;
        };
        let context = report_context.get_or_insert_with(|| Value::Object(Map::new()));
        let Some(context) = context.as_object_mut() else {
            return;
        };
        context.insert(
            CODEX_WEBSOCKET_RATE_LIMITS_REPORT_CONTEXT_FIELD.to_string(),
            rate_limits,
        );
    }

    fn observes_upstream_events(&self) -> bool {
        true
    }

    fn rebind_safety_for_upstream_event(&self, event: &Value) -> ResponsesWebSocketRebindSafety {
        let mut saw_event = false;
        if event.get("type").and_then(Value::as_str).is_some() {
            saw_event = true;
            let safety = codex_direct_rebind_safety(event);
            if matches!(safety, ResponsesWebSocketRebindSafety::Unsafe { .. }) {
                return safety;
            }
        }
        match event.get("chunks") {
            Some(Value::Array(chunks)) => {
                for chunk in chunks {
                    saw_event = true;
                    let safety = codex_direct_rebind_safety(chunk);
                    if matches!(safety, ResponsesWebSocketRebindSafety::Unsafe { .. }) {
                        return safety;
                    }
                }
            }
            Some(_) => {
                return ResponsesWebSocketRebindSafety::Unsafe {
                    reason: "unrecognized_upstream_event",
                };
            }
            None => {}
        }
        if saw_event {
            ResponsesWebSocketRebindSafety::Safe
        } else {
            ResponsesWebSocketRebindSafety::Unsafe {
                reason: "unrecognized_upstream_event",
            }
        }
    }

    fn relay_directive_for_upstream_event<'a>(
        &self,
        event: &'a Value,
    ) -> ResponsesWebSocketRelayDirective<'a> {
        codex_relay_directive(event)
    }

    fn observe_upstream_event(
        &self,
        event: &Value,
    ) -> Option<ResponsesWebSocketAdapterObservation> {
        let rate_limits = parse_codex_rate_limits(event)?;
        let account_exhausted =
            aether_admin::provider::quota::codex_rate_limit_metadata_exhausted(&rate_limits);
        let active_limit_exhausted =
            aether_admin::provider::quota::codex_websocket_response_has_usage_limit_error(event);
        let now_unix_secs = current_unix_secs();
        let scoped_reset_at = if account_exhausted {
            codex_quota_exhaustion_reset_at(&rate_limits, now_unix_secs)
        } else {
            codex_model_quota_exhaustion_reset_at(&rate_limits, now_unix_secs)
        };
        let retry_exclusion_until_unix_secs = active_limit_exhausted
            .then(|| {
                aether_admin::provider::quota::codex_websocket_usage_limit_reset_at(
                    event,
                    now_unix_secs,
                )
            })
            .flatten()
            .or(scoped_reset_at);
        Some(ResponsesWebSocketAdapterObservation {
            drain: (account_exhausted || active_limit_exhausted).then_some(
                ResponsesWebSocketDrainDirective {
                    error_code: if account_exhausted {
                        "codex_account_quota_exhausted"
                    } else {
                        "codex_active_limit_exhausted"
                    },
                    retry_current_turn: true,
                    retry_exclusion_until_unix_secs,
                },
            ),
            quota_metadata: Some(rate_limits),
        })
    }

    fn exhaustion_exclusion_identity(
        &self,
        decision: &AiExecutionDecision,
    ) -> Option<ResponsesWebSocketExclusionIdentity> {
        Some(ResponsesWebSocketExclusionIdentity {
            account_id: codex_account_id_from_headers(&decision.provider_request_headers)
                .map(str::to_string),
        })
    }

    async fn persist_upstream_observation(
        &self,
        state: &AppState,
        trace_id: &str,
        report_context: Option<&Value>,
        observation: ResponsesWebSocketAdapterObservation,
    ) {
        let Some(rate_limits) = observation.quota_metadata else {
            return;
        };
        if let Err(error) =
            sync_codex_websocket_quota_metadata(state, report_context, rate_limits).await
        {
            tracing::warn!(
                target: CODEX_WEBSOCKET_LOG_TARGET,
                event_name = "codex_websocket_quota_sync_failed",
                log_type = "ops",
                transport = "websocket",
                websocket = true,
                trace_id = %trace_id,
                error = ?error,
                "gateway failed to persist Codex WebSocket quota metadata"
            );
        }
    }
}

fn codex_direct_rebind_safety(event: &Value) -> ResponsesWebSocketRebindSafety {
    let event_type = event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if matches!(event_type, "codex.rate_limits" | "codex.response.metadata") {
        // Codex emits these as pre-response advisory metadata. They do
        // not create a public `response.*` object, so a replacement
        // upstream can safely emit its own current snapshot.
        return ResponsesWebSocketRebindSafety::Safe;
    }
    if event_type == "error"
        && event.pointer("/error/type").and_then(Value::as_str) == Some("usage_limit_reached")
        && parse_codex_rate_limits(event).is_some()
    {
        // This terminal quota event has not been relayed yet. It can trigger
        // one transparent attempt on another key as long as no earlier public
        // response event made the logical turn unsafe. If replanning fails,
        // the connection layer forwards this exact upstream error instead of
        // manufacturing a gateway continuation error.
        return ResponsesWebSocketRebindSafety::Safe;
    }
    let reason = if is_standard_responses_event(event) {
        "standard_response_event"
    } else {
        "unrecognized_upstream_event"
    };
    ResponsesWebSocketRebindSafety::Unsafe { reason }
}

fn codex_relay_directive(event: &Value) -> ResponsesWebSocketRelayDirective<'_> {
    match event.get("chunks") {
        Some(Value::Array(chunks)) if is_explicit_codex_batch_envelope(event) => {
            let public_events = chunks
                .iter()
                .filter_map(codex_public_event)
                .collect::<Vec<_>>();
            if public_events.is_empty() {
                ResponsesWebSocketRelayDirective::SuppressProviderPrivate
            } else {
                ResponsesWebSocketRelayDirective::ForwardEvents(public_events)
            }
        }
        // A malformed or future shape is not proven private. Preserve it
        // opaquely rather than guessing at a provider schema.
        Some(_) => ResponsesWebSocketRelayDirective::ForwardOriginal,
        None if is_codex_private_leaf_event(event) => match codex_public_event(event) {
            Some(projected) => ResponsesWebSocketRelayDirective::ForwardEvents(vec![projected]),
            None => ResponsesWebSocketRelayDirective::SuppressProviderPrivate,
        },
        None => ResponsesWebSocketRelayDirective::ForwardOriginal,
    }
}

fn codex_public_event(event: &Value) -> Option<Cow<'_, Value>> {
    if !is_codex_private_leaf_event(event) {
        return Some(Cow::Borrowed(event));
    }
    if event.get("type").and_then(Value::as_str) != Some("codex.response.metadata") {
        // 配额属于选中的上游账户，不能代表网关用户配额；仅交给
        // 账户级熔断和持久化路径处理。
        return None;
    }
    let headers = event.get("headers")?.as_object()?;
    let public_headers: Map<String, Value> = headers
        .iter()
        .filter_map(|(name, value)| {
            let name = name.to_ascii_lowercase();
            if matches!(
                name.as_str(),
                "x-models-etag"
                    | "x-codex-turn-state"
                    | "openai-model"
                    | "x-codex-safety-buffering-enabled"
                    | "x-codex-safety-buffering-faster-model"
            ) && value.as_str().is_some()
            {
                Some((name, value.clone()))
            } else {
                None
            }
        })
        .collect();
    if public_headers.is_empty() {
        return None;
    }
    Some(Cow::Owned(serde_json::json!({
        "type": "codex.response.metadata", "headers": public_headers
    })))
}

/// Recognizes only Codex's private batch container. A type-less object must
/// contain exactly `chunks`; unknown siblings could be future public protocol
/// data and therefore force opaque forwarding. A named Codex private root may
/// carry provider metadata alongside its chunks and is safe to peel.
fn is_explicit_codex_batch_envelope(event: &Value) -> bool {
    if is_codex_private_event_type(event) {
        return true;
    }
    event.as_object().is_some_and(|object| {
        object.len() == 1
            && object.contains_key("chunks")
            && event.get("type").and_then(Value::as_str).is_none()
    })
}

fn is_codex_private_leaf_event(event: &Value) -> bool {
    is_codex_private_event_type(event) && event.get("chunks").is_none()
}

fn is_codex_private_event_type(event: &Value) -> bool {
    matches!(
        event.get("type").and_then(Value::as_str),
        Some("codex.rate_limits" | "codex.response.metadata")
    )
}

fn parse_codex_rate_limits(event: &Value) -> Option<Value> {
    aether_admin::provider::quota::parse_codex_websocket_rate_limits_response(
        event,
        current_unix_secs(),
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        CodexResponsesWebSocketAdapter, ResponsesWebSocketProtocolAdapter,
        ResponsesWebSocketRebindSafety, ResponsesWebSocketRelayDirective,
    };

    #[test]
    fn codex_adapter_has_a_distinct_frontdoor_self_loop_error() {
        let adapter = CodexResponsesWebSocketAdapter;

        assert_eq!(
            adapter.upstream_errors().frontdoor_self_loop,
            "codex_websocket_frontdoor_self_loop"
        );
    }

    #[test]
    fn codex_rate_limit_chunk_is_kept_for_the_terminal_report() {
        let adapter = CodexResponsesWebSocketAdapter;
        assert!(adapter.observes_upstream_events());
        let mut context = Some(json!({"key_id": "codex-key"}));
        adapter.decorate_turn_report_context(
            &mut context,
            &json!({
                "chunks": [{
                    "type": "codex.rate_limits",
                    "plan_type": "free",
                    "rate_limits": {
                        "allowed": true,
                        "limit_reached": false,
                        "primary": {
                            "used_percent": 91,
                            "window_minutes": 43200,
                            "reset_after_seconds": 2590791
                        }
                    }
                }]
            }),
        );

        assert_eq!(
            context.as_ref().and_then(
                |context| context.pointer("/codex_websocket_rate_limits/primary_used_percent")
            ),
            Some(&json!(91.0))
        );
    }

    #[test]
    fn usage_limit_error_is_kept_for_the_terminal_report() {
        let adapter = CodexResponsesWebSocketAdapter;
        let mut context = Some(json!({"key_id": "codex-key"}));
        adapter.decorate_turn_report_context(
            &mut context,
            &json!({
                "type": "error",
                "error": {
                    "type": "usage_limit_reached",
                    "plan_type": "free",
                    "resets_at": 1_787_274_385u64,
                },
                "status_code": 429,
                "headers": {
                    "X-Codex-Primary-Used-Percent": "100",
                    "X-Codex-Primary-Reset-At": "1787274385",
                },
            }),
        );

        assert_eq!(
            context
                .as_ref()
                .and_then(|context| context.pointer("/codex_websocket_rate_limits/allowed")),
            Some(&json!(false))
        );
        assert_eq!(
            context.as_ref().and_then(|context| {
                context.pointer("/codex_websocket_rate_limits/primary_used_percent")
            }),
            Some(&json!(100.0))
        );
    }

    #[test]
    fn model_scoped_usage_limit_error_drains_without_account_exhaustion() {
        let adapter = CodexResponsesWebSocketAdapter;
        let event = json!({
            "type": "error",
            "error": {
                "type": "usage_limit_reached",
                "plan_type": "pro",
            },
            "status_code": 429,
            "headers": {
                "X-Codex-Plan-Type": "pro",
                "X-Codex-Active-Limit": "codex_bengalfox",
                "X-Codex-Primary-Used-Percent": "100",
                "X-Codex-Primary-Window-Minutes": "300",
                "X-Codex-Primary-Reset-At": "4000000000",
                "X-Codex-Bengalfox-Limit-Name": "GPT-5.3-Codex-Spark",
                "X-Codex-Bengalfox-Primary-Used-Percent": "100",
                "X-Codex-Bengalfox-Primary-Window-Minutes": "300",
                "X-Codex-Bengalfox-Primary-Reset-At": "4000000000",
            },
        });

        let observation = adapter
            .observe_upstream_event(&event)
            .expect("model-scoped quota error should be observed");
        let drain = observation
            .drain
            .expect("model-scoped quota error should retry the current turn");
        let quota = observation
            .quota_metadata
            .expect("model-scoped quota metadata should be retained");

        assert_eq!(drain.error_code, "codex_active_limit_exhausted");
        assert!(drain.retry_current_turn);
        assert_eq!(
            drain.retry_exclusion_until_unix_secs,
            Some(4_000_000_000u64)
        );
        assert_eq!(quota["spark_primary_used_percent"], json!(100.0));
        assert!(quota.get("allowed").is_none());
        assert!(quota.get("limit_reached").is_none());
        assert!(!aether_admin::provider::quota::codex_rate_limit_metadata_exhausted(&quota));
    }

    #[test]
    fn account_scoped_usage_limit_error_keeps_account_drain_semantics() {
        let adapter = CodexResponsesWebSocketAdapter;
        let event = json!({
            "type": "error",
            "error": {
                "type": "usage_limit_reached",
                "plan_type": "free",
                "resets_at": 4_000_000_000u64,
            },
            "status_code": 429,
            "headers": {
                "X-Codex-Plan-Type": "free",
                "X-Codex-Primary-Used-Percent": "100",
                "X-Codex-Primary-Window-Minutes": "43200",
                "X-Codex-Primary-Reset-At": "4000000000",
            },
        });

        let observation = adapter
            .observe_upstream_event(&event)
            .expect("account quota error should be observed");
        let drain = observation
            .drain
            .expect("account quota error should retry the current turn");
        let quota = observation
            .quota_metadata
            .expect("account quota metadata should be retained");

        assert_eq!(drain.error_code, "codex_account_quota_exhausted");
        assert!(drain.retry_current_turn);
        assert_eq!(
            drain.retry_exclusion_until_unix_secs,
            Some(4_000_000_000u64)
        );
        assert_eq!(quota["allowed"], json!(false));
        assert_eq!(quota["limit_reached"], json!(true));
        assert!(aether_admin::provider::quota::codex_rate_limit_metadata_exhausted(&quota));
    }

    #[test]
    fn only_known_codex_pre_response_signals_are_safe_to_rebind() {
        let adapter = CodexResponsesWebSocketAdapter;

        assert_eq!(
            adapter.rebind_safety_for_upstream_event(&json!({
                "type": "codex.rate_limits",
                "rate_limits": {"allowed": true}
            })),
            ResponsesWebSocketRebindSafety::Safe
        );
        assert_eq!(
            adapter.rebind_safety_for_upstream_event(&json!({
                "type": "codex.response.metadata"
            })),
            ResponsesWebSocketRebindSafety::Safe
        );
        assert_eq!(
            adapter.rebind_safety_for_upstream_event(&json!({
                "chunks": [
                    {"type": "codex.rate_limits", "rate_limits": {"allowed": true}},
                    {"type": "codex.response.metadata"}
                ]
            })),
            ResponsesWebSocketRebindSafety::Safe
        );
        assert_eq!(
            adapter.rebind_safety_for_upstream_event(&json!({
                "type": "response.created"
            })),
            ResponsesWebSocketRebindSafety::Unsafe {
                reason: "standard_response_event"
            }
        );
        assert_eq!(
            adapter.rebind_safety_for_upstream_event(&json!({
                "type": "codex.unknown"
            })),
            ResponsesWebSocketRebindSafety::Unsafe {
                reason: "unrecognized_upstream_event"
            }
        );
        assert_eq!(
            adapter.rebind_safety_for_upstream_event(&json!({
                "type": "error",
                "error": {
                    "type": "usage_limit_reached",
                    "plan_type": "plus",
                    "resets_in_seconds": 3_600
                },
                "status_code": 429
            })),
            ResponsesWebSocketRebindSafety::Safe
        );
        assert_eq!(
            adapter.rebind_safety_for_upstream_event(&json!({
                "type": "error",
                "error": {"type": "usage_limit_reached"}
            })),
            ResponsesWebSocketRebindSafety::Unsafe {
                reason: "unrecognized_upstream_event"
            }
        );
        assert_eq!(
            adapter.rebind_safety_for_upstream_event(&json!({
                "type": "response.future_capability.delta",
                "chunks": [{"type": "codex.rate_limits"}]
            })),
            ResponsesWebSocketRebindSafety::Unsafe {
                reason: "standard_response_event"
            }
        );
    }

    #[test]
    fn codex_suppresses_only_explicit_private_events_and_envelopes() {
        let adapter = CodexResponsesWebSocketAdapter;

        for event in [
            json!({"type": "codex.rate_limits", "rate_limits": {"allowed": true}}),
            json!({"type": "codex.response.metadata", "account_hint": "private"}),
            json!({"chunks": [
                {"type": "codex.rate_limits"},
                {"type": "codex.response.metadata"}
            ]}),
        ] {
            assert_eq!(
                adapter.relay_directive_for_upstream_event(&event),
                ResponsesWebSocketRelayDirective::SuppressProviderPrivate
            );
        }

        for event in [
            json!({"type": "error", "error": {"type": "usage_limit_reached"}}),
            json!({"type": "codex.future_private_maybe", "future": true}),
            json!({"chunks": [], "future_envelope_field": {"must": "survive"}}),
            json!({"type": "response.future.done", "future_capability": true}),
        ] {
            assert_eq!(
                adapter.relay_directive_for_upstream_event(&event),
                ResponsesWebSocketRelayDirective::ForwardOriginal
            );
        }
    }

    #[test]
    fn codex_metadata_relays_cli_catalog_and_turn_state_without_account_fields() {
        let event = json!({
            "type": "codex.response.metadata",
            "headers": {
                "X-Models-Etag": "catalog-v2",
                "x-codex-turn-state": "synthetic-turn-state",
                "openai-model": "gpt-6.1-sol",
                "x-codex-safety-buffering-enabled": "true",
                "x-codex-safety-buffering-faster-model": "gpt-6-luna",
                "chatgpt-account-id": "synthetic-private-account",
                "set-cookie": "synthetic-private-cookie",
                "authorization": "synthetic-private-token"
            },
            "account_hint": "private",
            "metadata": {"user_id": "private"}
        });
        let ResponsesWebSocketRelayDirective::ForwardEvents(events) =
            CodexResponsesWebSocketAdapter.relay_directive_for_upstream_event(&event)
        else {
            panic!("CLI metadata must reach the client");
        };
        assert_eq!(events.len(), 1);
        assert_eq!(
            *events[0],
            json!({
                "type": "codex.response.metadata",
                "headers": {"x-models-etag": "catalog-v2", "x-codex-turn-state": "synthetic-turn-state", "openai-model": "gpt-6.1-sol", "x-codex-safety-buffering-enabled": "true", "x-codex-safety-buffering-faster-model": "gpt-6-luna"}
            })
        );
    }

    #[test]
    fn codex_batch_retains_safe_metadata_in_public_event_order() {
        let event = json!({"chunks": [
            {"type": "codex.response.metadata", "headers": {"x-models-etag": "catalog-v3"}},
            {"type": "codex.rate_limits", "plan_type": "private-plan"},
            {"type": "response.created", "response": {"id": "resp_synthetic"}, "future": 42}
        ]});
        let ResponsesWebSocketRelayDirective::ForwardEvents(events) =
            CodexResponsesWebSocketAdapter.relay_directive_for_upstream_event(&event)
        else {
            panic!("batch must retain public events");
        };
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["headers"]["x-models-etag"], "catalog-v3");
        assert_eq!(events[1]["future"], 42);
    }

    #[test]
    fn mixed_codex_batch_forwards_whole_non_private_events_in_order() {
        let adapter = CodexResponsesWebSocketAdapter;
        let event = json!({
            "chunks": [
                {
                    "type": "response.created",
                    "response": {"id": "resp_future"},
                    "future_created_field": {"opaque": true}
                },
                {"type": "codex.rate_limits", "account_hint": "private"},
                {
                    "type": "response.future_capability.delta",
                    "future_capability": {"nested": [1, 2, 3]},
                    "sequence_number": 2
                },
                {"provider_future_event": {"unknown": "must be forwarded"}},
                {
                    "type": "error",
                    "error": {"type": "future_error", "future_detail": 7}
                }
            ]
        });

        let ResponsesWebSocketRelayDirective::ForwardEvents(events) =
            adapter.relay_directive_for_upstream_event(&event)
        else {
            panic!("a mixed private envelope must retain all non-private events");
        };
        assert_eq!(events.len(), 4);
        assert_eq!(events[0]["future_created_field"], json!({"opaque": true}));
        assert_eq!(events[1]["future_capability"], json!({"nested": [1, 2, 3]}));
        assert_eq!(
            events[2]["provider_future_event"],
            json!({"unknown": "must be forwarded"})
        );
        assert_eq!(events[3]["error"]["future_detail"], json!(7));
    }
}
