//! Physical upstream WebSocket binding and transport helpers.

use std::time::Duration;

use serde_json::Value;
use wreq::ws::message::Message as WreqWsMessage;

use super::adapter::ResponsesWebSocketProtocolAdapter;
use super::binding::{UpstreamBindingIdentity, UpstreamBindingIdentityError};
use super::plan_admission::responses_websocket_plan_permit_is_healthy;
use super::redaction::ResponsesWebSocketRedactionRestorer;
use super::request::{planned_request_uses_codex_responses_lite, planned_response_create_event};
use super::state::{
    BoundResponsesConnection, ContinuationResponseIds, ExhaustedResponsesWebSocketExclusions,
};
use super::turn::UpstreamRequestState;
use super::turn_state::ResponsesTurnState;
use crate::ai_serving::{AiExecutionDecision, ResponsesWebSocketBodyNormalization};
use crate::handlers::proxy::websocket::session::RESPONSES_WEBSOCKET_SESSION_LIMITS;
use crate::handlers::proxy::websocket::transport::{
    close_upstream_socket, connect_upstream_websocket, feed_upstream_message,
    flush_upstream_messages, WebSocketWriteError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResponsesWebSocketUpstreamSendError {
    PlanUsagePermitLost,
    Transport(WebSocketWriteError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResponsesWebSocketUpstreamBindError {
    PlanUsagePermitLost,
    Transport(&'static str),
}

/// 上游 WebSocket 握手的默认绝对 deadline（30 秒）。
/// 覆盖 DNS → TCP connect → TLS → HTTP 101 Upgrade → 发送首条 event 的完整链路。
/// 如果 decision 配置了更短的 first_byte_ms 或 total_ms，取其与此值的较小者。
const DEFAULT_UPSTREAM_HANDSHAKE_DEADLINE_MS: u64 = 30_000;

/// 从 decision.timeouts 推导实际 handshake 绝对 deadline。
/// 取 first_byte_ms / total_ms / DEFAULT 三者中的最小正值。
pub(super) fn resolve_upstream_handshake_deadline(decision: &AiExecutionDecision) -> Duration {
    let mut deadline_ms = DEFAULT_UPSTREAM_HANDSHAKE_DEADLINE_MS;
    if let Some(timeouts) = decision.timeouts.as_ref() {
        if let Some(first_byte_ms) = timeouts.first_byte_ms.filter(|v| *v > 0) {
            deadline_ms = deadline_ms.min(first_byte_ms);
        }
        if let Some(total_ms) = timeouts.total_ms.filter(|v| *v > 0) {
            deadline_ms = deadline_ms.min(total_ms);
        }
    }
    Duration::from_millis(deadline_ms)
}

pub(super) async fn bind_responses_upstream<F>(
    decision: &AiExecutionDecision,
    normalization: ResponsesWebSocketBodyNormalization,
    initial_event: &Value,
    adapter: &'static dyn ResponsesWebSocketProtocolAdapter,
    plan_usage_permit: Option<&aether_runtime::AdmissionPermit>,
    record_upstream_request_state: F,
) -> Result<BoundResponsesConnection, ResponsesWebSocketUpstreamBindError>
where
    F: FnMut(UpstreamRequestState),
{
    // 绝对 deadline：从此刻起必须在限定时间内完成握手 + 首条事件发送，
    // 防止慢 TLS / 慢 HTTP Upgrade 无限占用 connection permit。
    let handshake_deadline = resolve_upstream_handshake_deadline(decision);
    tokio::time::timeout(
        handshake_deadline,
        bind_responses_upstream_inner(
            decision,
            normalization,
            initial_event,
            adapter,
            plan_usage_permit,
            record_upstream_request_state,
        ),
    )
    .await
    .map_err(|_| {
        ResponsesWebSocketUpstreamBindError::Transport(
            "responses_websocket_upstream_handshake_timeout",
        )
    })?
}

/// 实际执行握手 + 首条事件发送的内部函数，由外层 timeout 包裹。
async fn bind_responses_upstream_inner<F>(
    decision: &AiExecutionDecision,
    normalization: ResponsesWebSocketBodyNormalization,
    initial_event: &Value,
    adapter: &'static dyn ResponsesWebSocketProtocolAdapter,
    plan_usage_permit: Option<&aether_runtime::AdmissionPermit>,
    record_upstream_request_state: F,
) -> Result<BoundResponsesConnection, ResponsesWebSocketUpstreamBindError>
where
    F: FnMut(UpstreamRequestState),
{
    let binding_identity =
        UpstreamBindingIdentity::from_decision(adapter, decision).map_err(|error| {
            ResponsesWebSocketUpstreamBindError::Transport(match error {
                UpstreamBindingIdentityError::MissingUpstreamUrl => {
                    adapter.upstream_errors().upstream_url_missing
                }
                UpstreamBindingIdentityError::InvalidUpstreamUrl => {
                    adapter.upstream_errors().upstream_url_invalid
                }
                UpstreamBindingIdentityError::InvalidHandshakeHeaders => {
                    adapter.upstream_errors().headers_invalid
                }
            })
        })?;
    let mut upstream = connect_upstream_websocket(
        decision,
        RESPONSES_WEBSOCKET_SESSION_LIMITS,
        adapter.upstream_errors(),
    )
    .await
    .map_err(ResponsesWebSocketUpstreamBindError::Transport)?;
    let first_event = planned_response_create_event(decision, &normalization, initial_event)
        .map_err(ResponsesWebSocketUpstreamBindError::Transport)?;
    send_responses_websocket_upstream_message(
        &mut upstream.socket,
        WreqWsMessage::text(first_event),
        plan_usage_permit,
        record_upstream_request_state,
    )
    .await
    .map_err(|error| match error {
        ResponsesWebSocketUpstreamSendError::PlanUsagePermitLost => {
            ResponsesWebSocketUpstreamBindError::PlanUsagePermitLost
        }
        ResponsesWebSocketUpstreamSendError::Transport(_) => {
            ResponsesWebSocketUpstreamBindError::Transport(
                "responses_websocket_initial_send_failed",
            )
        }
    })?;

    let client_model = initial_event
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(ResponsesWebSocketUpstreamBindError::Transport(
            "responses_websocket_model_missing",
        ))?
        .to_string();
    let provider_model = decision
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
        })
        .ok_or(ResponsesWebSocketUpstreamBindError::Transport(
            "responses_websocket_mapped_model_missing",
        ))?
        .to_string();

    let responses_lite_static_config =
        planned_request_uses_codex_responses_lite(decision, &normalization).then(|| {
            super::request::ResponsesLiteStaticConfig::from_response_create(initial_event)
        });

    Ok(BoundResponsesConnection {
        upstream: Some(upstream.socket),
        adapter,
        client_model,
        provider_model,
        decision_template: decision.clone(),
        body_normalization: normalization,
        responses_lite_static_config,
        binding_identity,
        continuation_response_ids: ContinuationResponseIds::default(),
        // 首条 response.create 已经发出，但这一轮的 logical turn 和 attempt 由调用方
        // 通过 `ResponsesTurnState::begin` 装上：绑定本身不持有记账状态。
        turn_state: ResponsesTurnState::Idle,
        // 同理，这一轮的 mask session 也由调用方登记：绑定看不到脱敏链路。
        redaction_restorer: ResponsesWebSocketRedactionRestorer::default(),
        next_turn_index: 2,
        upstream_response_headers: upstream.response_headers,
        pending_adapter_drain: None,
        pending_adapter_observation: None,
        exhausted_exclusions: ExhaustedResponsesWebSocketExclusions::default(),
        pending_turn_finalization: None,
    })
}

pub(super) async fn send_responses_websocket_upstream_message<F>(
    upstream: &mut wreq::ws::WebSocket,
    message: WreqWsMessage,
    plan_usage_permit: Option<&aether_runtime::AdmissionPermit>,
    record_upstream_request_state: F,
) -> Result<(), ResponsesWebSocketUpstreamSendError>
where
    F: FnMut(UpstreamRequestState),
{
    let mut record_upstream_request_state = record_upstream_request_state;
    if !responses_websocket_plan_permit_is_healthy(plan_usage_permit) {
        return Err(ResponsesWebSocketUpstreamSendError::PlanUsagePermitLost);
    }
    feed_upstream_message(upstream, message)
        .await
        .map_err(ResponsesWebSocketUpstreamSendError::Transport)?;
    // No await between successful start_send and the state transition: an
    // outer supervisor cannot cancel this attempt in the ambiguity window.
    record_upstream_request_state(UpstreamRequestState::PossiblySent);
    flush_upstream_messages(upstream)
        .await
        .map_err(ResponsesWebSocketUpstreamSendError::Transport)?;
    record_upstream_request_state(UpstreamRequestState::Sent);
    Ok(())
}

#[cfg(test)]
async fn complete_responses_websocket_upstream_send<Q, F, R>(
    queue: Q,
    flush: F,
    plan_usage_permit: Option<&aether_runtime::AdmissionPermit>,
    mut record_upstream_request_state: R,
) -> Result<(), ResponsesWebSocketUpstreamSendError>
where
    Q: std::future::Future<Output = Result<(), WebSocketWriteError>>,
    F: std::future::Future<Output = Result<(), WebSocketWriteError>>,
    R: FnMut(UpstreamRequestState),
{
    // Concurrency lease health is an admission condition, so check it before
    // the socket writer is polled. Once polling begins, SinkExt::send may have
    // completed start_send while still waiting for flush; cancelling it at
    // that point cannot prove the provider did not receive response.create.
    if !responses_websocket_plan_permit_is_healthy(plan_usage_permit) {
        return Err(ResponsesWebSocketUpstreamSendError::PlanUsagePermitLost);
    }
    queue
        .await
        .map_err(ResponsesWebSocketUpstreamSendError::Transport)?;
    // No await between successful start_send and the state transition: an
    // outer supervisor cannot cancel this attempt in the ambiguity window.
    record_upstream_request_state(UpstreamRequestState::PossiblySent);
    flush
        .await
        .map_err(ResponsesWebSocketUpstreamSendError::Transport)?;
    record_upstream_request_state(UpstreamRequestState::Sent);
    Ok(())
}

pub(super) async fn receive_optional_upstream(
    upstream: &mut Option<wreq::ws::WebSocket>,
) -> Option<Result<WreqWsMessage, ()>> {
    match upstream.as_mut() {
        Some(upstream) => upstream.recv().await.map(|message| message.map_err(|_| ())),
        None => std::future::pending().await,
    }
}

pub(super) async fn close_bound_upstream(bound: &mut BoundResponsesConnection) {
    if let Some(mut upstream) = bound.upstream.take() {
        close_upstream_socket(&mut upstream, None).await;
    }
}

pub(super) fn decision_reuses_bound_upstream(
    bound: &BoundResponsesConnection,
    adapter: &'static dyn ResponsesWebSocketProtocolAdapter,
    decision: &AiExecutionDecision,
) -> bool {
    bound.upstream.is_some()
        && UpstreamBindingIdentity::from_decision(adapter, decision)
            .map(|identity| bound.binding_identity == identity)
            .unwrap_or(false)
}

pub(super) fn decision_bound_upstream_change_fields(
    bound: &BoundResponsesConnection,
    adapter: &'static dyn ResponsesWebSocketProtocolAdapter,
    decision: &AiExecutionDecision,
) -> Vec<String> {
    if bound.upstream.is_none() {
        return vec!["upstream_socket".to_string()];
    }
    match UpstreamBindingIdentity::from_decision(adapter, decision) {
        Ok(identity) => bound.binding_identity.changed_field_names(&identity),
        Err(_) => vec!["binding_identity_invalid".to_string()],
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::task::{Context, Poll};
    use std::time::Duration;

    use aether_contracts::ExecutionTimeouts;

    use crate::ai_serving::AiExecutionDecision;

    use super::{
        complete_responses_websocket_upstream_send, resolve_upstream_handshake_deadline,
        ResponsesWebSocketUpstreamBindError, DEFAULT_UPSTREAM_HANDSHAKE_DEADLINE_MS,
    };
    use crate::handlers::proxy::websocket::responses::turn::UpstreamRequestState;
    use crate::handlers::proxy::websocket::transport::WebSocketWriteError;

    struct ReadySendThatRequestsSupervisorCancellation {
        cancellation_requested: Arc<AtomicBool>,
    }

    struct MutablePermitHealth(Arc<AtomicBool>);

    impl aether_runtime::AdmissionPermitHealth for MutablePermitHealth {
        fn is_healthy(&self) -> bool {
            self.0.load(Ordering::Acquire)
        }
    }

    impl Future for ReadySendThatRequestsSupervisorCancellation {
        type Output = Result<(), WebSocketWriteError>;

        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
            // Model the socket writer waking an already-running outer
            // supervisor in the same poll in which it reports a successful
            // provider transfer. The supervisor cannot drop this future until
            // control returns from this poll.
            self.cancellation_requested.store(true, Ordering::Release);
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn successful_send_marks_attempt_before_returning_to_cancelling_supervisor() {
        let cancellation_requested = Arc::new(AtomicBool::new(false));
        let upstream_request_sent = Arc::new(AtomicBool::new(false));
        let state_by_handoff = Arc::clone(&upstream_request_sent);

        let result = complete_responses_websocket_upstream_send(
            ReadySendThatRequestsSupervisorCancellation {
                cancellation_requested: Arc::clone(&cancellation_requested),
            },
            async { Ok(()) },
            None,
            move |state| {
                if state == UpstreamRequestState::Sent {
                    state_by_handoff.store(true, Ordering::Release);
                }
            },
        )
        .await;

        assert_eq!(result, Ok(()));
        assert!(cancellation_requested.load(Ordering::Acquire));
        assert!(
            upstream_request_sent.load(Ordering::Acquire),
            "a successful provider write must transfer lifecycle ownership before an outer supervisor can cancel the send future"
        );
    }

    #[tokio::test]
    async fn unhealthy_plan_permit_rejects_before_polling_the_upstream_send() {
        let healthy = Arc::new(AtomicBool::new(false));
        let permit =
            aether_runtime::AdmissionPermit::from_parts(None, Some(MutablePermitHealth(healthy)))
                .expect("test permit");
        let send_polled = Arc::new(AtomicBool::new(false));
        let send_polled_by_future = Arc::clone(&send_polled);

        let result = complete_responses_websocket_upstream_send(
            async move {
                send_polled_by_future.store(true, Ordering::Release);
                Ok(())
            },
            async { Ok(()) },
            Some(&permit),
            |_| {},
        )
        .await;

        assert_eq!(
            result,
            Err(super::ResponsesWebSocketUpstreamSendError::PlanUsagePermitLost)
        );
        assert!(!send_polled.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn plan_permit_loss_after_send_start_does_not_cancel_the_send() {
        let healthy = Arc::new(AtomicBool::new(true));
        let permit = aether_runtime::AdmissionPermit::from_parts(
            None,
            Some(MutablePermitHealth(Arc::clone(&healthy))),
        )
        .expect("test permit");
        let flush_started = Arc::new(tokio::sync::Notify::new());
        let allow_flush_to_finish = Arc::new(tokio::sync::Notify::new());
        let flush_started_by_future = Arc::clone(&flush_started);
        let allow_flush_to_finish_by_future = Arc::clone(&allow_flush_to_finish);
        let marked_possibly_sent = Arc::new(AtomicBool::new(false));
        let marked_possibly_sent_by_callback = Arc::clone(&marked_possibly_sent);
        let marked_sent = Arc::new(AtomicBool::new(false));
        let marked_sent_by_callback = Arc::clone(&marked_sent);

        let send = complete_responses_websocket_upstream_send(
            async { Ok(()) },
            async move {
                flush_started_by_future.notify_one();
                allow_flush_to_finish_by_future.notified().await;
                Ok(())
            },
            Some(&permit),
            move |state| match state {
                UpstreamRequestState::PossiblySent => {
                    marked_possibly_sent_by_callback.store(true, Ordering::Release)
                }
                UpstreamRequestState::Sent => {
                    marked_sent_by_callback.store(true, Ordering::Release)
                }
                UpstreamRequestState::NotStarted => {}
            },
        );
        let revoke = async move {
            flush_started.notified().await;
            healthy.store(false, Ordering::Release);
            allow_flush_to_finish.notify_one();
        };
        let (result, ()) = tokio::join!(send, revoke);

        assert_eq!(result, Ok(()));
        assert!(marked_possibly_sent.load(Ordering::Acquire));
        assert!(marked_sent.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn flush_failure_keeps_the_attempt_possibly_sent() {
        let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed_by_callback = Arc::clone(&observed);

        let result = complete_responses_websocket_upstream_send(
            async { Ok(()) },
            async { Err(WebSocketWriteError::TimedOut) },
            None,
            move |state| observed_by_callback.lock().expect("states").push(state),
        )
        .await;

        assert_eq!(
            result,
            Err(super::ResponsesWebSocketUpstreamSendError::Transport(
                WebSocketWriteError::TimedOut
            ))
        );
        assert_eq!(
            *observed.lock().expect("states"),
            vec![UpstreamRequestState::PossiblySent]
        );
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
            extra_headers: std::collections::BTreeMap::new(),
            provider_request_headers: std::collections::BTreeMap::new(),
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

    #[test]
    fn handshake_deadline_defaults_to_30s_without_configured_timeouts() {
        let decision = sample_decision();
        let deadline = resolve_upstream_handshake_deadline(&decision);
        assert_eq!(
            deadline,
            Duration::from_millis(DEFAULT_UPSTREAM_HANDSHAKE_DEADLINE_MS)
        );
    }

    #[test]
    fn handshake_deadline_uses_first_byte_ms_when_shorter_than_default() {
        let mut decision = sample_decision();
        decision.timeouts = Some(ExecutionTimeouts {
            first_byte_ms: Some(10_000),
            total_ms: Some(60_000),
            ..ExecutionTimeouts::default()
        });
        let deadline = resolve_upstream_handshake_deadline(&decision);
        assert_eq!(deadline, Duration::from_millis(10_000));
    }

    #[test]
    fn handshake_deadline_uses_total_ms_when_shorter_than_first_byte_and_default() {
        let mut decision = sample_decision();
        decision.timeouts = Some(ExecutionTimeouts {
            first_byte_ms: Some(25_000),
            total_ms: Some(8_000),
            ..ExecutionTimeouts::default()
        });
        let deadline = resolve_upstream_handshake_deadline(&decision);
        assert_eq!(deadline, Duration::from_millis(8_000));
    }

    #[test]
    fn handshake_deadline_ignores_zero_values() {
        let mut decision = sample_decision();
        decision.timeouts = Some(ExecutionTimeouts {
            first_byte_ms: Some(0),
            total_ms: Some(0),
            ..ExecutionTimeouts::default()
        });
        let deadline = resolve_upstream_handshake_deadline(&decision);
        assert_eq!(
            deadline,
            Duration::from_millis(DEFAULT_UPSTREAM_HANDSHAKE_DEADLINE_MS)
        );
    }

    #[test]
    fn handshake_deadline_does_not_exceed_default_even_with_larger_configured_values() {
        let mut decision = sample_decision();
        decision.timeouts = Some(ExecutionTimeouts {
            first_byte_ms: Some(120_000),
            total_ms: Some(600_000),
            ..ExecutionTimeouts::default()
        });
        let deadline = resolve_upstream_handshake_deadline(&decision);
        assert_eq!(
            deadline,
            Duration::from_millis(DEFAULT_UPSTREAM_HANDSHAKE_DEADLINE_MS)
        );
    }

    #[tokio::test]
    async fn bind_responses_upstream_times_out_against_stalled_server() {
        use super::bind_responses_upstream;
        use crate::ai_serving::ResponsesWebSocketBodyNormalization;
        use crate::handlers::proxy::websocket::responses::adapter::resolve_responses_websocket_adapter;
        use serde_json::json;

        // 启动一个接受 TCP 连接但永不完成 HTTP Upgrade 的 mock 服务器
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock listener should bind");
        let addr = listener.local_addr().expect("should have local addr");
        let _server = tokio::spawn(async move {
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                // 接受连接但不发送任何 HTTP 响应，模拟 stalled handshake
                tokio::spawn(async move {
                    let _hold = socket;
                    tokio::time::sleep(Duration::from_secs(300)).await;
                });
            }
        });

        let mut decision = sample_decision();
        decision.upstream_url = Some(format!("http://{addr}/v1/responses"));
        // 设置极短的 deadline 以便测试快速完成
        decision.timeouts = Some(ExecutionTimeouts {
            first_byte_ms: Some(100),
            total_ms: Some(200),
            ..ExecutionTimeouts::default()
        });
        decision.provider_request_body = Some(json!({"model": "test-model"}));

        let adapter = resolve_responses_websocket_adapter(
            crate::orchestration::ResponsesWebSocketAdapter::Standard,
        );
        let result = bind_responses_upstream(
            &decision,
            ResponsesWebSocketBodyNormalization::for_tests("test-model"),
            &json!({"type": "response.create", "model": "test-model"}),
            adapter,
            None,
            |_| {},
        )
        .await;

        assert_eq!(
            result.err().expect("bind should fail with timeout"),
            ResponsesWebSocketUpstreamBindError::Transport(
                "responses_websocket_upstream_handshake_timeout"
            )
        );
    }
}
