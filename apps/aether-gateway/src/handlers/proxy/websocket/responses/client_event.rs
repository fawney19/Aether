use serde_json::Value;

use super::request::{
    validate_response_create_previous_response_id, validate_response_create_stream_id_support,
    validated_named_stream_id, MAX_RESPONSES_WEBSOCKET_RESPONSE_ID_BYTES,
};

#[derive(Debug)]
pub(super) enum ResponsesClientEvent<'a> {
    Create(Value),
    Cancel(ResponseCancel<'a>),
}

#[derive(Debug)]
pub(super) struct ResponseCancel<'a> {
    pub(super) raw_text: &'a str,
    pub(super) response_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ClientEventParseError {
    pub(super) code: &'static str,
    pub(super) stream_id: Option<String>,
}

impl ClientEventParseError {
    fn new(code: &'static str, event: Option<&Value>) -> Self {
        let stream_id = event
            .and_then(validated_named_stream_id)
            .map(str::to_string);
        Self { code, stream_id }
    }

    pub(super) fn message(&self) -> &'static str {
        match self.code {
            "invalid_response_create_previous_response_id" => {
                "response.create.previous_response_id must be null or a non-empty string"
            }
            "invalid_response_create_stream_id" => {
                "response.create.stream_id must be 1-256 ASCII letters, numbers, underscores, hyphens, or periods"
            }
            "invalid_response_cancel_stream_id" => {
                "response.cancel.stream_id must be 1-256 ASCII letters, numbers, underscores, hyphens, or periods"
            }
            "responses_websocket_named_stream_unsupported" => {
                "Aether currently supports only the implicit default WebSocket lane; omit stream_id"
            }
            "invalid_response_cancel_response_id" => {
                "response.cancel.response_id must be a non-empty string of at most 256 bytes"
            }
            _ => "WebSocket client text events must be response.create or response.cancel JSON objects",
        }
    }
}

pub(super) fn parse_client_event(
    text: &str,
) -> Result<ResponsesClientEvent<'_>, ClientEventParseError> {
    let event = serde_json::from_str::<Value>(text)
        .map_err(|_| ClientEventParseError::new("invalid_response_create", None))?;
    if !event.is_object() {
        return Err(ClientEventParseError::new(
            "invalid_response_create",
            Some(&event),
        ));
    }
    match event.get("type").and_then(Value::as_str) {
        Some("response.create") => {
            validate_response_create_previous_response_id(&event)
                .map_err(|code| ClientEventParseError::new(code, Some(&event)))?;
            validate_response_create_stream_id_support(&event)
                .map_err(|code| ClientEventParseError::new(code, Some(&event)))?;
            Ok(ResponsesClientEvent::Create(event))
        }
        Some("response.cancel") => {
            parse_response_cancel(&event, text).map(ResponsesClientEvent::Cancel)
        }
        _ => Err(ClientEventParseError::new(
            "expected_response_create",
            Some(&event),
        )),
    }
}

pub(super) fn parse_response_cancel<'a>(
    event: &Value,
    text: &'a str,
) -> Result<ResponseCancel<'a>, ClientEventParseError> {
    validate_response_create_stream_id_support(event).map_err(|code| {
        ClientEventParseError::new(
            if code == "invalid_response_create_stream_id" {
                "invalid_response_cancel_stream_id"
            } else {
                code
            },
            Some(event),
        )
    })?;
    let response_id = match event.get("response_id") {
        None => None,
        Some(Value::String(id))
            if !id.trim().is_empty() && id.len() <= MAX_RESPONSES_WEBSOCKET_RESPONSE_ID_BYTES =>
        {
            Some(id.clone())
        }
        Some(_) => {
            return Err(ClientEventParseError::new(
                "invalid_response_cancel_response_id",
                Some(event),
            ));
        }
    };
    Ok(ResponseCancel {
        raw_text: text,
        response_id,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        parse_client_event, ResponsesClientEvent, MAX_RESPONSES_WEBSOCKET_RESPONSE_ID_BYTES,
    };

    #[test]
    fn cancel_preserves_the_raw_control_frame_and_optional_identity() {
        for (text, expected_id) in [
            (
                r#"{ "type": "response.cancel", "event_id": "cancel-1" }"#,
                None,
            ),
            (
                r#"{"type":"response.cancel","response_id":"resp-1","future":true}"#,
                Some("resp-1"),
            ),
        ] {
            let ResponsesClientEvent::Cancel(cancel) =
                parse_client_event(text).expect("valid cancel")
            else {
                panic!("cancel must not enter response.create planning");
            };
            assert_eq!(cancel.raw_text, text);
            assert_eq!(cancel.response_id.as_deref(), expected_id);
        }
    }

    #[test]
    fn cancel_rejects_malformed_identity_and_unsupported_lanes() {
        for response_id in [
            json!(null),
            json!(42),
            json!(""),
            json!(" "),
            json!({"id": "resp-1"}),
            json!("x".repeat(MAX_RESPONSES_WEBSOCKET_RESPONSE_ID_BYTES + 1)),
        ] {
            let text = json!({"type": "response.cancel", "response_id": response_id}).to_string();
            assert_eq!(
                parse_client_event(&text).expect_err("invalid id").code,
                "invalid_response_cancel_response_id"
            );
        }
        let named = parse_client_event(r#"{"type":"response.cancel","stream_id":"main"}"#)
            .expect_err("named lane is unsupported");
        assert_eq!(named.code, "responses_websocket_named_stream_unsupported");
        assert_eq!(named.stream_id.as_deref(), Some("main"));
        let invalid = parse_client_event(r#"{"type":"response.cancel","stream_id":"bad/lane"}"#)
            .expect_err("invalid lane");
        assert_eq!(invalid.code, "invalid_response_cancel_stream_id");
        assert_eq!(invalid.stream_id, None);
    }

    #[test]
    fn invalid_client_text_does_not_poison_the_next_response_create() {
        for (text, code) in [
            ("not-json", "invalid_response_create"),
            ("[]", "invalid_response_create"),
            (
                r#"{"type":"response.unsupported"}"#,
                "expected_response_create",
            ),
            (
                r#"{"type":"response.create","previous_response_id":""}"#,
                "invalid_response_create_previous_response_id",
            ),
            (
                r#"{"type":"response.create","previous_response_id":42}"#,
                "invalid_response_create_previous_response_id",
            ),
            (
                r#"{"type":"response.create","previous_response_id":{"id":"resp_1"}}"#,
                "invalid_response_create_previous_response_id",
            ),
        ] {
            assert_eq!(
                parse_client_event(text).expect_err("invalid create").code,
                code
            );
        }
        let invalid_previous = parse_client_event(
            r#"{"type":"response.create","stream_id":"main","previous_response_id":""}"#,
        )
        .expect_err("invalid previous id retains valid lane identity");
        assert_eq!(
            invalid_previous.code,
            "invalid_response_create_previous_response_id"
        );
        assert_eq!(invalid_previous.stream_id.as_deref(), Some("main"));
        let named =
            parse_client_event(r#"{"type":"response.create","stream_id":"main-lane_1.test"}"#)
                .expect_err("named streams are not implemented");
        assert_eq!(named.code, "responses_websocket_named_stream_unsupported");
        assert_eq!(named.stream_id.as_deref(), Some("main-lane_1.test"));
        for text in [
            r#"{"type":"response.create","stream_id":null}"#,
            r#"{"type":"response.create","stream_id":""}"#,
            r#"{"type":"response.create","stream_id":"not/a/lane"}"#,
        ] {
            let error = parse_client_event(text).expect_err("invalid lane");
            assert_eq!(error.code, "invalid_response_create_stream_id");
            assert_eq!(error.stream_id, None);
        }
        let ResponsesClientEvent::Create(valid) =
            parse_client_event(r#"{"type":"response.create","model":"gpt-test","store":true}"#)
                .expect("valid create after rejected event")
        else {
            panic!("expected create");
        };
        assert_eq!(valid["type"], "response.create");
        assert_eq!(valid["store"], true);
    }
}
