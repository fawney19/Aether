//! Parsed OpenAI Responses WebSocket text frames.
//!
//! A relay frame is parsed once and then shared by the protocol adapter, turn
//! accounting, retry safety, and connection lifecycle code.  Keeping the raw
//! text as a borrow avoids copying the websocket payload while the relay is
//! processing it.

use serde_json::Value;

use super::request::MAX_RESPONSES_WEBSOCKET_RESPONSE_ID_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ResponsesWebSocketFrameTerminal {
    pub(super) status_code: u16,
    pub(super) cancelled: bool,
}

#[derive(Debug)]
pub(super) struct ParsedResponsesWebSocketFrame<'a> {
    raw_text: &'a str,
    event: Value,
    event_type: Option<String>,
    status: Option<u16>,
    started: bool,
    terminal: Option<ResponsesWebSocketFrameTerminal>,
    terminal_event: Option<Value>,
    chunked: bool,
}

impl<'a> ParsedResponsesWebSocketFrame<'a> {
    pub(super) fn parse(raw_text: &'a str) -> serde_json::Result<Self> {
        let event = serde_json::from_str::<Value>(raw_text)?;
        let events = protocol_events_of(&event);
        let started = events.iter().copied().any(event_is_started);
        // A batch carries at most one terminal in practice.  Taking the first
        // in document order keeps the outcome deterministic if that ever
        // stops being true.
        let terminal_entry = events
            .iter()
            .copied()
            .find_map(|candidate| terminal_for_event(candidate).map(|term| (candidate, term)));
        let terminal = terminal_entry.map(|(_, terminal)| terminal);
        // The terminal event describes the turn's outcome, so it is the one
        // worth naming in logs and recording as the terminal error body.
        let event_type = terminal_entry
            .map(|(candidate, _)| candidate)
            .or_else(|| events.last().copied())
            .and_then(event_type_of)
            .map(str::to_string);
        let terminal_event = terminal_entry.map(|(candidate, _)| candidate.clone());
        let chunked = event.get("chunks").and_then(Value::as_array).is_some();
        let status = terminal.map(|terminal| terminal.status_code);

        Ok(Self {
            raw_text,
            event,
            event_type,
            status,
            started,
            terminal,
            terminal_event,
            chunked,
        })
    }

    /// The protocol events this frame carries.
    ///
    /// Codex batches standard `response.*` events into a `{"chunks":[...]}`
    /// envelope, so one frame can carry several events — and the terminal one
    /// may be buried inside the batch.  Every consumer that interprets event
    /// semantics must walk this rather than the envelope, or a batched
    /// `response.completed` goes unnoticed and wedges the turn.
    pub(super) fn protocol_events(&self) -> Vec<&Value> {
        protocol_events_of(&self.event)
    }

    /// The individual event that ended the turn, unwrapped from its batch.
    pub(super) fn terminal_event(&self) -> Option<&Value> {
        self.terminal_event.as_ref()
    }

    pub(super) fn is_chunked(&self) -> bool {
        self.chunked
    }

    pub(super) fn raw_text(&self) -> &'a str {
        self.raw_text
    }

    pub(super) fn event(&self) -> &Value {
        &self.event
    }

    /// Whether the provider attached a named-lane identity to this frame.
    ///
    /// Aether's current relay owns only the implicit default lane. OpenAI's
    /// named-lane contract places `stream_id` directly on each public event;
    /// Codex may additionally batch those events under `chunks`. Inspect both
    /// wire positions without looking through arbitrary response payloads so
    /// a model-produced field named `stream_id` is not mistaken for transport
    /// framing.
    pub(super) fn carries_stream_id(&self) -> bool {
        self.event.get("stream_id").is_some()
            || self
                .event
                .get("chunks")
                .and_then(Value::as_array)
                .is_some_and(|chunks| chunks.iter().any(|event| event.get("stream_id").is_some()))
    }

    pub(super) fn event_type(&self) -> Option<&str> {
        self.event_type.as_deref()
    }

    pub(super) fn status(&self) -> Option<u16> {
        self.status
    }

    pub(super) fn is_started(&self) -> bool {
        self.started
    }

    pub(super) fn is_terminal(&self) -> bool {
        self.terminal.is_some()
    }

    pub(super) fn terminal(&self) -> Option<ResponsesWebSocketFrameTerminal> {
        self.terminal
    }

    /// Returns the opaque response ID only for a successfully observed,
    /// provider-persistable terminal. Failed, cancelled, malformed and
    /// oversized terminal IDs must never establish continuation ownership.
    pub(super) fn continuation_response_id(&self) -> Option<&str> {
        let terminal = self.terminal?;
        if terminal.status_code >= 400 || terminal.cancelled {
            return None;
        }
        let event = self.terminal_event.as_ref()?;
        if !matches!(
            event_type_of(event),
            Some("response.completed" | "response.done" | "response.incomplete")
        ) {
            return None;
        }
        let response_id = event
            .pointer("/response/id")
            .or_else(|| event.get("response_id"))
            .and_then(Value::as_str)?;
        (!response_id.trim().is_empty()
            && response_id.len() <= MAX_RESPONSES_WEBSOCKET_RESPONSE_ID_BYTES)
            .then_some(response_id)
    }

    /// Return a bounded label suitable for structured logs.  Event payloads
    /// are never inserted directly into a log field.
    pub(super) fn event_type_for_log(&self) -> String {
        self.event_type
            .as_deref()
            .map(safe_websocket_event_label)
            .unwrap_or_else(|| "invalid_json".to_string())
    }
}

/// Encodes one event peeled from a provider-private envelope without applying
/// an event-type or field projection.
///
/// Direct provider events should use [`ParsedResponsesWebSocketFrame::raw_text`]
/// so their bytes remain identical. This helper exists only for batch
/// envelopes that cannot be relayed as a whole: serializing the complete
/// [`Value`] preserves every known and future JSON member.
pub(super) fn encode_opaque_websocket_event(event: &Value) -> serde_json::Result<String> {
    serde_json::to_string(event)
}

/// Flattens a frame into the events it carries.  An envelope may name its own
/// `type` *and* batch further events under `chunks`; both are protocol events.
fn protocol_events_of(event: &Value) -> Vec<&Value> {
    let mut events = Vec::new();
    if event_type_of(event).is_some() {
        events.push(event);
    }
    if let Some(chunks) = event.get("chunks").and_then(Value::as_array) {
        events.extend(chunks.iter().filter(|chunk| event_type_of(chunk).is_some()));
    }
    // An unrecognized shape is still relayed and still accounted for, so it
    // must not vanish from the observer's view of the stream.
    if events.is_empty() {
        events.push(event);
    }
    events
}

fn event_type_of(event: &Value) -> Option<&str> {
    event.get("type").and_then(Value::as_str)
}

fn event_is_started(event: &Value) -> bool {
    matches!(
        event_type_of(event).unwrap_or_default(),
        "response.created" | "response.in_progress" | "response.queued"
    )
}

/// 读取 `response.incomplete` 携带的 `incomplete_details.reason`。
///
/// 标准位置是 `response.incomplete_details.reason`；批量封装偶尔把
/// `incomplete_details` 直接放在事件顶层，两处都要看，否则合法终态会被漏判。
fn responses_incomplete_reason(event: &Value) -> Option<&str> {
    [
        event.pointer("/response/incomplete_details/reason"),
        event.pointer("/incomplete_details/reason"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .map(str::trim)
    .find(|reason| !reason.is_empty())
}

fn responses_incomplete_has_explicit_error(event: &Value) -> bool {
    [event.get("error"), event.pointer("/response/error")]
        .into_iter()
        .flatten()
        .any(|error| !error.is_null())
}

/// Derives only the fallback status for `response.incomplete`.
///
/// A non-empty reason is provider-owned protocol data. Treating it as a fixed
/// allowlist would turn every future legitimate reason into a synthetic 502
/// and incorrectly penalize provider health. Missing/malformed reasons and
/// explicit error markers still fail closed; numeric status and recognized
/// error codes continue to override this fallback in
/// [`websocket_event_status_code`].
fn responses_incomplete_default_status(event: &Value) -> u16 {
    match responses_incomplete_reason(event) {
        None => 502,
        Some(reason)
            if reason.eq_ignore_ascii_case("error")
                || reason.eq_ignore_ascii_case("server_error") =>
        {
            502
        }
        Some(_) if responses_incomplete_has_explicit_error(event) => 502,
        Some(_) => 200,
    }
}

fn terminal_for_event(event: &Value) -> Option<ResponsesWebSocketFrameTerminal> {
    match event_type_of(event).unwrap_or_default() {
        "response.completed" | "response.done"
            if event.pointer("/response/status").and_then(Value::as_str) == Some("cancelled") =>
        {
            Some(ResponsesWebSocketFrameTerminal {
                status_code: 499,
                cancelled: true,
            })
        }
        "response.completed" | "response.done" => Some(ResponsesWebSocketFrameTerminal {
            status_code: websocket_event_status_code(event, 200),
            cancelled: false,
        }),
        // A non-empty provider reason is a normal terminal by default, including
        // future reasons Aether does not yet know. Explicit status/error data
        // still wins, so quota and server failures retain their failure status.
        "response.incomplete" => Some(ResponsesWebSocketFrameTerminal {
            status_code: websocket_event_status_code(
                event,
                responses_incomplete_default_status(event),
            ),
            cancelled: false,
        }),
        "response.cancelled" => Some(ResponsesWebSocketFrameTerminal {
            status_code: 499,
            cancelled: true,
        }),
        "response.failed" => Some(ResponsesWebSocketFrameTerminal {
            status_code: websocket_event_status_code(event, 502),
            cancelled: false,
        }),
        "error" => Some(ResponsesWebSocketFrameTerminal {
            status_code: websocket_event_status_code(event, 502),
            cancelled: false,
        }),
        _ => None,
    }
}

fn websocket_event_status_code(event: &Value, default: u16) -> u16 {
    if let Some(status_code) = event
        .get("status_code")
        .or_else(|| event.get("status"))
        .or_else(|| {
            event
                .get("response")
                .and_then(|response| response.get("status_code"))
        })
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .filter(|value| *value > 0)
    {
        return status_code;
    }

    let error_code = [
        event.pointer("/error/type"),
        event.pointer("/error/code"),
        event.pointer("/response/error/type"),
        event.pointer("/response/error/code"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .map(str::to_ascii_lowercase)
    .find(|value| !value.trim().is_empty());
    match error_code.as_deref() {
        Some(
            "usage_limit_reached" | "insufficient_quota" | "rate_limit_exceeded" | "quota_exceeded",
        ) => 429,
        Some("invalid_api_key" | "authentication_error") => 401,
        Some("invalid_request_error" | "invalid_request" | "model_not_found") => 400,
        Some("overloaded" | "server_error" | "service_unavailable") => 503,
        _ => default,
    }
}

fn safe_websocket_event_label(value: &str) -> String {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 80
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return "unknown".to_string();
    }
    value.to_string()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{encode_opaque_websocket_event, ParsedResponsesWebSocketFrame};

    #[test]
    fn parses_started_frame_once_with_raw_text_and_event_metadata() {
        let raw = r#"{"type":"response.in_progress","response":{"status":200}}"#;
        let frame = ParsedResponsesWebSocketFrame::parse(raw).expect("valid frame");

        assert_eq!(frame.raw_text(), raw);
        assert_eq!(frame.event_type(), Some("response.in_progress"));
        assert_eq!(frame.status(), None);
        assert!(frame.is_started());
        assert!(!frame.is_terminal());
        assert_eq!(frame.event()["response"]["status"], 200);
        assert_eq!(frame.event_type_for_log(), "response.in_progress");
    }

    #[test]
    fn continuation_id_requires_a_successful_persistable_terminal() {
        for event_type in ["response.completed", "response.done"] {
            let raw = format!(r#"{{"type":"{event_type}","response":{{"id":"resp_persisted"}}}}"#);
            let frame = ParsedResponsesWebSocketFrame::parse(&raw).expect("valid terminal");
            assert_eq!(frame.continuation_response_id(), Some("resp_persisted"));
        }
        let incomplete = ParsedResponsesWebSocketFrame::parse(
            r#"{"type":"response.incomplete","response":{"id":"resp_partial","incomplete_details":{"reason":"max_output_tokens"}}}"#,
        )
        .expect("valid incomplete terminal");
        assert_eq!(incomplete.continuation_response_id(), Some("resp_partial"));

        for raw in [
            r#"{"type":"response.failed","response":{"id":"resp_failed"}}"#,
            r#"{"type":"response.cancelled","response":{"id":"resp_cancelled"}}"#,
            r#"{"type":"response.completed","status_code":500,"response":{"id":"resp_error"}}"#,
        ] {
            let frame = ParsedResponsesWebSocketFrame::parse(raw).expect("valid terminal");
            assert_eq!(frame.continuation_response_id(), None);
        }
    }

    #[test]
    fn future_response_event_keeps_its_exact_original_text_and_unknown_fields() {
        let raw = "{ \n  \"future_top_level\": {\"nested\": [1, true, null]}, \n  \"type\": \"response.future_capability.delta\", \n  \"delta\": {\"new_wire_shape\": \"opaque\"}\n}";
        let frame = ParsedResponsesWebSocketFrame::parse(raw).expect("valid future event");

        assert_eq!(frame.raw_text(), raw);
        assert_eq!(
            frame.event()["future_top_level"],
            json!({"nested": [1, true, null]})
        );
        assert_eq!(frame.event()["delta"], json!({"new_wire_shape": "opaque"}));
        assert!(!frame.is_terminal());
    }

    #[test]
    fn peeled_batch_event_encoding_preserves_the_complete_opaque_value() {
        let frame = ParsedResponsesWebSocketFrame::parse(
            r#"{"chunks":[{"type":"response.future.done","future_capability":{"mode":"new"},"response":{"id":"resp_future","future_usage":{"novel_tokens":7}}}]}"#,
        )
        .expect("valid private envelope");
        let events = frame.protocol_events();
        let event = events.first().expect("one future response event");
        let encoded = encode_opaque_websocket_event(event).expect("Value serialization succeeds");
        let round_trip: serde_json::Value =
            serde_json::from_str(&encoded).expect("encoded event stays valid JSON");

        assert_eq!(round_trip, **event);
        assert_eq!(round_trip["future_capability"], json!({"mode": "new"}));
        assert_eq!(round_trip["response"]["future_usage"]["novel_tokens"], 7);
    }

    #[test]
    fn detects_a_top_level_named_lane_identity() {
        let frame = ParsedResponsesWebSocketFrame::parse(
            r#"{"type":"response.in_progress","stream_id":"planner","response":{"id":"resp_1"}}"#,
        )
        .expect("valid named-lane event");

        assert!(frame.carries_stream_id());
    }

    #[test]
    fn detects_a_named_lane_identity_inside_a_batch() {
        let frame = ParsedResponsesWebSocketFrame::parse(
            r#"{"type":"codex.response.metadata","chunks":[{"type":"codex.rate_limits"},{"type":"response.completed","stream_id":"planner","response":{"id":"resp_1"}}]}"#,
        )
        .expect("valid named-lane batch");

        assert!(frame.carries_stream_id());
    }

    #[test]
    fn response_payload_fields_are_not_mistaken_for_lane_framing() {
        let frame = ParsedResponsesWebSocketFrame::parse(
            r#"{"type":"response.completed","response":{"id":"resp_1","metadata":{"stream_id":"model-owned"}}}"#,
        )
        .expect("valid default-lane event");

        assert!(!frame.carries_stream_id());
    }

    #[test]
    fn classifies_terminal_status_and_cancellation() {
        let completed = ParsedResponsesWebSocketFrame::parse(
            r#"{"type":"response.completed","status_code":201}"#,
        )
        .expect("valid frame");
        assert_eq!(completed.status(), Some(201));
        assert_eq!(
            completed
                .terminal()
                .map(|terminal| (terminal.status_code, terminal.cancelled)),
            Some((201, false))
        );

        let cancelled = ParsedResponsesWebSocketFrame::parse(r#"{"type":"response.cancelled"}"#)
            .expect("valid frame");
        assert_eq!(cancelled.status(), Some(499));
        assert_eq!(
            cancelled
                .terminal()
                .map(|terminal| (terminal.status_code, terminal.cancelled)),
            Some((499, true))
        );

        for event_type in ["response.done", "response.completed"] {
            let text = format!(
                r#"{{"type":"{event_type}","response":{{"id":"resp_cancelled","status":"cancelled"}}}}"#
            );
            let cancelled =
                ParsedResponsesWebSocketFrame::parse(&text).expect("valid cancelled terminal");
            assert_eq!(
                cancelled
                    .terminal()
                    .map(|terminal| (terminal.status_code, terminal.cancelled)),
                Some((499, true))
            );
            assert_eq!(cancelled.continuation_response_id(), None);
        }

        let error = ParsedResponsesWebSocketFrame::parse(
            r#"{"type":"error","status_code":429,"error":{"type":"usage_limit_reached"}}"#,
        )
        .expect("valid frame");
        assert_eq!(error.status(), Some(429));
        assert!(error.is_terminal());

        let failed = ParsedResponsesWebSocketFrame::parse(
            r#"{"type":"response.failed","response":{"error":{"code":"rate_limit_exceeded"}}}"#,
        )
        .expect("valid frame");
        assert_eq!(failed.status(), Some(429));
    }

    #[test]
    fn a_legitimate_incomplete_is_a_terminal_but_not_a_provider_failure() {
        for reason in [
            "max_output_tokens",
            "max_tokens",
            "content_filter",
            "tool_calls",
            "function_call",
            "MAX_OUTPUT_TOKENS",
        ] {
            let raw = format!(
                r#"{{"type":"response.incomplete","response":{{"status":"incomplete","incomplete_details":{{"reason":"{reason}"}}}}}}"#
            );
            let frame = ParsedResponsesWebSocketFrame::parse(&raw).expect("valid frame");

            assert!(frame.is_terminal(), "{reason} should end the turn");
            assert_eq!(
                frame
                    .terminal()
                    .map(|terminal| (terminal.status_code, terminal.cancelled)),
                Some((200, false)),
                "{reason} is a legitimate terminal result, not a 502 provider failure"
            );
        }
    }

    #[test]
    fn a_top_level_incomplete_details_reason_is_also_honored() {
        let frame = ParsedResponsesWebSocketFrame::parse(
            r#"{"type":"response.incomplete","incomplete_details":{"reason":"max_output_tokens"}}"#,
        )
        .expect("valid frame");

        assert_eq!(frame.status(), Some(200));
    }

    #[test]
    fn an_incomplete_without_a_reason_or_with_a_failure_reason_stays_a_provider_failure() {
        for raw in [
            r#"{"type":"response.incomplete"}"#,
            r#"{"type":"response.incomplete","response":{"incomplete_details":null}}"#,
            r#"{"type":"response.incomplete","response":{"incomplete_details":{"reason":""}}}"#,
            r#"{"type":"response.incomplete","response":{"incomplete_details":{"reason":"error"}}}"#,
            r#"{"type":"response.incomplete","response":{"incomplete_details":{"reason":"server_error"}}}"#,
        ] {
            let frame = ParsedResponsesWebSocketFrame::parse(raw).expect("valid frame");

            assert_eq!(
                frame.status(),
                Some(502),
                "an incomplete without a usable reason must stay a provider failure: {raw}"
            );
        }
    }

    #[test]
    fn a_future_incomplete_reason_is_forward_compatible_without_hiding_explicit_errors() {
        let future = ParsedResponsesWebSocketFrame::parse(
            r#"{"type":"response.incomplete","response":{"incomplete_details":{"reason":"future_context_boundary"}}}"#,
        )
        .expect("valid frame");
        assert_eq!(future.status(), Some(200));

        let future_with_error = ParsedResponsesWebSocketFrame::parse(
            r#"{"type":"response.incomplete","response":{"error":{"code":"future_provider_error"},"incomplete_details":{"reason":"future_context_boundary"}}}"#,
        )
        .expect("valid frame");
        assert_eq!(future_with_error.status(), Some(502));
    }

    #[test]
    fn a_legitimate_incomplete_still_respects_an_explicit_provider_status() {
        let explicit = ParsedResponsesWebSocketFrame::parse(
            r#"{"type":"response.incomplete","status_code":503,"response":{"incomplete_details":{"reason":"max_output_tokens"}}}"#,
        )
        .expect("valid frame");
        assert_eq!(explicit.status(), Some(503));

        let quota = ParsedResponsesWebSocketFrame::parse(
            r#"{"type":"response.incomplete","response":{"error":{"code":"rate_limit_exceeded"},"incomplete_details":{"reason":"max_output_tokens"}}}"#,
        )
        .expect("valid frame");
        assert_eq!(quota.status(), Some(429));
    }

    #[test]
    fn a_legitimate_incomplete_batched_inside_a_chunks_envelope_is_not_a_failure() {
        let frame = ParsedResponsesWebSocketFrame::parse(
            r#"{"chunks":[{"type":"response.output_text.delta","delta":"hi"},{"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"},"usage":{"total_tokens":9}}}]}"#,
        )
        .expect("valid frame");

        assert!(frame.is_chunked());
        assert!(frame.is_terminal());
        assert_eq!(frame.status(), Some(200));
        assert_eq!(frame.event_type(), Some("response.incomplete"));
        assert_eq!(
            frame.terminal_event().and_then(|event| event
                .pointer("/response/usage/total_tokens")
                .and_then(serde_json::Value::as_u64)),
            Some(9)
        );
    }

    #[test]
    fn detects_a_terminal_batched_inside_a_chunks_envelope() {
        let frame = ParsedResponsesWebSocketFrame::parse(
            r#"{"chunks":[{"type":"response.output_text.delta","delta":"hi"},{"type":"response.completed","response":{"usage":{"total_tokens":8}}}]}"#,
        )
        .expect("valid frame");

        assert!(frame.is_chunked());
        assert!(frame.is_terminal());
        assert_eq!(frame.status(), Some(200));
        // The label and the recorded error body must name the event that ended
        // the turn, not the envelope.
        assert_eq!(frame.event_type(), Some("response.completed"));
        assert_eq!(
            frame.terminal_event().and_then(|event| event
                .pointer("/response/usage/total_tokens")
                .and_then(serde_json::Value::as_u64)),
            Some(8)
        );
        assert_eq!(frame.protocol_events().len(), 2);
    }

    #[test]
    fn detects_a_start_event_batched_inside_a_chunks_envelope() {
        let frame = ParsedResponsesWebSocketFrame::parse(
            r#"{"chunks":[{"type":"codex.rate_limits"},{"type":"response.created"}]}"#,
        )
        .expect("valid frame");

        assert!(frame.is_started());
        assert!(!frame.is_terminal());
        assert_eq!(frame.protocol_events().len(), 2);
    }

    #[test]
    fn an_envelope_may_carry_its_own_type_alongside_batched_events() {
        let frame = ParsedResponsesWebSocketFrame::parse(
            r#"{"type":"codex.response.metadata","chunks":[{"type":"response.failed","response":{"error":{"code":"rate_limit_exceeded"}}}]}"#,
        )
        .expect("valid frame");

        assert_eq!(frame.protocol_events().len(), 2);
        assert!(frame.is_terminal());
        assert_eq!(frame.status(), Some(429));
        assert_eq!(frame.event_type(), Some("response.failed"));
    }

    #[test]
    fn a_batch_without_a_terminal_does_not_end_the_turn() {
        let frame = ParsedResponsesWebSocketFrame::parse(
            r#"{"chunks":[{"type":"response.output_text.delta","delta":"a"},{"type":"response.output_text.delta","delta":"b"}]}"#,
        )
        .expect("valid frame");

        assert!(!frame.is_terminal());
        assert!(!frame.is_started());
        assert!(frame.terminal_event().is_none());
    }

    #[test]
    fn an_unrecognized_shape_is_still_surfaced_as_one_event() {
        let frame =
            ParsedResponsesWebSocketFrame::parse(r#"{"unexpected":true}"#).expect("valid frame");

        assert_eq!(frame.protocol_events().len(), 1);
        assert!(!frame.is_chunked());
        assert!(!frame.is_terminal());
        assert_eq!(frame.event_type(), None);
        assert_eq!(frame.event_type_for_log(), "invalid_json");
    }

    #[test]
    fn preserves_safe_log_label_boundaries() {
        let unsafe_label =
            ParsedResponsesWebSocketFrame::parse(r#"{"type":"not safe / contains spaces"}"#)
                .expect("valid frame");
        assert_eq!(unsafe_label.event_type_for_log(), "unknown");

        let missing_label =
            ParsedResponsesWebSocketFrame::parse(r#"{"message":"ok"}"#).expect("valid frame");
        assert_eq!(missing_label.event_type_for_log(), "invalid_json");
    }

    #[test]
    fn rejects_invalid_json() {
        assert!(ParsedResponsesWebSocketFrame::parse("not-json").is_err());
    }
}
