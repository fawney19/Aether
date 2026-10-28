use super::*;

const CANCEL_INPUT_TOKENS: u64 = 9;
const CANCEL_OUTPUT_TOKENS: u64 = 3;

#[tokio::test]
async fn active_cancel_without_response_id_is_forwarded_and_socket_reused() -> Result<(), BoxError>
{
    Box::pin(assert_cancel_and_reuse(
        r#"{ "type": "response.cancel", "event_id": "cancel-current" }"#,
        "response.cancelled",
        ProviderFixture::SingleOpenAiKey,
    ))
    .await
}

#[tokio::test]
async fn active_cancel_with_matching_response_id_is_forwarded_and_socket_reused(
) -> Result<(), BoxError> {
    Box::pin(assert_cancel_and_reuse(
        r#"{ "type": "response.cancel", "response_id": "resp-e2e-1", "event_id": "cancel-matching" }"#,
        "response.cancelled",
        ProviderFixture::SingleOpenAiKey,
    ))
    .await
}

#[tokio::test]
async fn active_cancel_done_alias_settles_cancelled_usage_and_socket_reused() -> Result<(), BoxError>
{
    Box::pin(assert_cancel_and_reuse(
        r#"{"type":"response.cancel","event_id":"cancel-done-alias"}"#,
        "response.done",
        ProviderFixture::SingleOpenAiKey,
    ))
    .await
}

#[tokio::test]
async fn codex_cancel_bills_tokens_and_reuses_one_upstream_connection() -> Result<(), BoxError> {
    Box::pin(assert_cancel_and_reuse(
        r#"{ "type": "response.cancel", "response_id": "resp-e2e-1", "event_id": "cancel-codex" }"#,
        "response.cancelled",
        ProviderFixture::SingleCodexKey,
    ))
    .await
}

async fn assert_cancel_and_reuse(
    cancel_frame: &str,
    terminal: &str,
    fixture: ProviderFixture,
) -> Result<(), BoxError> {
    let harness = Box::pin(Harness::start_configured(
        UpstreamBehavior::CancelFirstTurnThenComplete,
        fixture,
        PiiRedaction::Disabled,
        false,
        ModelPricing::PerToken {
            input_per_million: 2.0,
            output_per_million: 5.0,
        },
    ))
    .await?;
    let mut client = harness.connect().await?;
    client
        .send(response_create(json!({"input": "cancel this turn"})))
        .await?;
    receive_event(&mut client, "response.output_text.delta").await?;

    client
        .send(Message::Text(cancel_frame.to_owned().into()))
        .await?;

    let cancelled = receive_event(&mut client, terminal).await?;
    assert_eq!(cancelled["response"]["id"], "resp-e2e-1");
    assert_eq!(cancelled["response"]["status"], "cancelled");
    assert_eq!(
        cancelled["response"]["usage"],
        json!({"input_tokens": 9, "output_tokens": 3, "total_tokens": 12})
    );
    assert_eq!(
        *harness.upstream.cancel_frames.lock().await,
        vec![cancel_frame]
    );

    client
        .send(response_create(
            json!({"input": "continue after cancellation"}),
        ))
        .await?;
    let completed = receive_event(&mut client, "response.completed").await?;
    assert_eq!(completed["response"]["id"], "resp-e2e-2");
    assert_eq!(harness.upstream.connections(), 1);
    assert_eq!(harness.upstream.observed_events().await.len(), 2);
    let audits = harness
        .usage_audits_where(2, "settled cancelled and completed turns", |audit| {
            audit.billing_status == "settled"
                && audit.is_websocket()
                && audit.total_tokens > 0
                && audit.actual_total_cost_usd > 0.0
        })
        .await?;
    assert_eq!(
        audits.len(),
        2,
        "cancel must not create another provider attempt"
    );
    let cancelled_audit = audits
        .iter()
        .find(|audit| audit.status == "cancelled")
        .ok_or("cancelled turn did not persist cancelled usage")?;
    assert_eq!(cancelled_audit.input_tokens, CANCEL_INPUT_TOKENS);
    assert_eq!(cancelled_audit.output_tokens, CANCEL_OUTPUT_TOKENS);
    assert_eq!(
        cancelled_audit.total_tokens,
        CANCEL_INPUT_TOKENS + CANCEL_OUTPUT_TOKENS
    );
    let completed_audit = audits
        .iter()
        .find(|audit| audit.status == "completed")
        .ok_or("next turn did not persist completed usage")?;
    assert_eq!(completed_audit.input_tokens, INPUT_TOKENS);
    assert_eq!(completed_audit.output_tokens, OUTPUT_TOKENS);
    assert_ne!(cancelled_audit.request_id, completed_audit.request_id);
    for (audit, expected_cost) in [(cancelled_audit, 0.000033), (completed_audit, 0.000018)] {
        for cost in [audit.total_cost_usd, audit.actual_total_cost_usd] {
            assert!(
                (cost - expected_cost).abs() < 1e-12,
                "token cost: {audit:?}"
            );
        }
    }
    client.close(None).await?;
    Ok(())
}

#[tokio::test]
async fn idle_cancel_is_recoverable_without_creating_an_upstream_attempt() -> Result<(), BoxError> {
    Box::pin(assert_idle_cancel_is_recoverable(true)).await
}

#[tokio::test]
async fn initial_idle_cancel_is_recoverable_without_opening_upstream() -> Result<(), BoxError> {
    Box::pin(assert_idle_cancel_is_recoverable(false)).await
}

async fn assert_idle_cancel_is_recoverable(bind_first: bool) -> Result<(), BoxError> {
    let harness = Box::pin(Harness::start(UpstreamBehavior::CompleteEveryTurn)).await?;
    let mut client = harness.connect().await?;
    if bind_first {
        client
            .send(response_create(json!({"input": "bind socket"})))
            .await?;
        receive_event(&mut client, "response.completed").await?;
    }

    client
        .send(Message::Text(
            json!({"type": "response.cancel"}).to_string().into(),
        ))
        .await?;

    let error = receive_error_or_close(&mut client)
        .await?
        .ok_or("idle cancel closed the client socket")?;
    assert_eq!(error["status"], 400);
    assert_eq!(error["error"]["code"], "response_cancel_not_active");
    assert_eq!(harness.upstream.connections(), usize::from(bind_first));
    assert_eq!(
        harness.upstream.observed_events().await.len(),
        usize::from(bind_first)
    );
    if !bind_first {
        assert!(harness.usage_audits().await?.is_empty());
    }
    client
        .send(response_create(json!({"input": "after idle cancel"})))
        .await?;
    receive_event(&mut client, "response.completed").await?;
    let expected_turns = usize::from(bind_first) + 1;
    assert_eq!(
        harness.upstream.observed_events().await.len(),
        expected_turns
    );
    assert_eq!(harness.upstream.connections(), 1);
    assert!(harness.upstream.cancel_frames.lock().await.is_empty());
    let audits = harness
        .usage_audits_where(expected_turns, "valid turns around idle cancel", is_billed)
        .await?;
    assert_eq!(audits.len(), expected_turns);
    client.close(None).await?;
    Ok(())
}

#[tokio::test]
async fn mismatched_cancel_is_recoverable_without_cancelling_the_active_response(
) -> Result<(), BoxError> {
    let harness = Harness::start(UpstreamBehavior::CancelFirstTurnThenComplete).await?;
    let mut client = harness.connect().await?;
    client
        .send(response_create(json!({"input": "active turn"})))
        .await?;
    receive_event(&mut client, "response.output_text.delta").await?;

    client
        .send(Message::Text(
            json!({
                "type": "response.cancel", "response_id": "resp-other"
            })
            .to_string()
            .into(),
        ))
        .await?;

    let error = receive_error_or_close(&mut client)
        .await?
        .ok_or("mismatched cancel closed the client socket")?;
    assert_eq!(error["status"], 400);
    assert_eq!(
        error["error"]["code"],
        "response_cancel_response_id_mismatch"
    );
    assert!(harness.upstream.cancel_frames.lock().await.is_empty());
    let valid_cancel = r#"{"type":"response.cancel","response_id":"resp-e2e-1"}"#;
    client.send(Message::Text(valid_cancel.into())).await?;
    receive_event(&mut client, "response.cancelled").await?;
    assert_eq!(
        *harness.upstream.cancel_frames.lock().await,
        vec![valid_cancel]
    );
    let audits = harness
        .usage_audits_where(1, "cancelled active turn", |audit| {
            audit.status == "cancelled" && audit.billing_status == "settled"
        })
        .await?;
    assert_eq!(audits.len(), 1);
    assert_eq!(harness.upstream.observed_events().await.len(), 1);
    client.close(None).await?;
    Ok(())
}

pub(super) async fn cancel_first_turn(
    socket: &mut WebSocket,
    state: &MockUpstreamState,
) -> Result<(), axum::Error> {
    send_mock_created(socket, "resp-e2e-1").await?;
    send_mock_event(
        socket,
        json!({
            "type": "response.output_text.delta", "response_id": "resp-e2e-1", "delta": "partial"
        }),
    )
    .await?;
    let Some(Ok(AxumWsMessage::Text(frame))) = socket.recv().await else {
        return Ok(());
    };
    state.cancel_frames.lock().await.push(frame.to_string());
    let event: Value = serde_json::from_str(frame.as_str()).expect("cancel is a JSON event");
    assert_eq!(event["type"], "response.cancel");
    if let Some(response_id) = event.get("response_id") {
        assert_eq!(response_id, "resp-e2e-1");
    }
    let terminal = match event.get("event_id").and_then(Value::as_str) {
        Some("cancel-done-alias") => "response.done",
        _ => "response.cancelled",
    };
    send_mock_event(
        socket,
        json!({
            "type": terminal,
            "response": {
                "id": "resp-e2e-1", "status": "cancelled", "model": UPSTREAM_MODEL,
                "output": [],
                "usage": {
                    "input_tokens": CANCEL_INPUT_TOKENS,
                    "output_tokens": CANCEL_OUTPUT_TOKENS,
                    "total_tokens": CANCEL_INPUT_TOKENS + CANCEL_OUTPUT_TOKENS
                }
            }
        }),
    )
    .await
}
