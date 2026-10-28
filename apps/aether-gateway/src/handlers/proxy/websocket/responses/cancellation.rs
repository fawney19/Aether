use axum::extract::ws::WebSocket;
use wreq::ws::message::Message as WreqWsMessage;

use super::client::RelayDisposition;
use super::client_event::ResponseCancel;
use super::quota::mark_active_response_retry_unsafe;
use super::state::BoundResponsesConnection;
use crate::handlers::proxy::websocket::transport::{send_gateway_error, send_upstream_message};

pub(super) async fn forward_response_cancel(
    cancel: ResponseCancel<'_>,
    bound: &mut BoundResponsesConnection,
    client_socket: &mut WebSocket,
) -> RelayDisposition {
    let Some(attempt) = bound.turn_state.attempt() else {
        send_gateway_error(
            client_socket,
            "response_cancel_not_active",
            "There is no active response to cancel",
        )
        .await;
        return RelayDisposition::Continue;
    };
    if cancel
        .response_id
        .as_deref()
        .is_some_and(|response_id| attempt.response_id() != Some(response_id))
    {
        send_gateway_error(
            client_socket,
            "response_cancel_response_id_mismatch",
            "response.cancel.response_id does not identify the active response",
        )
        .await;
        return RelayDisposition::Continue;
    }

    // Cancellation may race a provider terminal. Do not replay this attempt or
    // synthesize a terminal: only the provider can report its final usage.
    mark_active_response_retry_unsafe(bound, "client_response_cancel");
    let Some(upstream) = bound.upstream.as_mut() else {
        return RelayDisposition::UpstreamError("upstream_unavailable_during_cancel");
    };
    match send_upstream_message(upstream, WreqWsMessage::text(cancel.raw_text)).await {
        Ok(()) => RelayDisposition::Continue,
        Err(_) => RelayDisposition::UpstreamError("upstream_cancel_send_failed"),
    }
}
