use serde_json::{Map, Value};

use super::auth::GeminiCliRequestAuth;

#[derive(Debug, Clone, PartialEq)]
pub enum GeminiCliRequestEnvelopeSupport {
    Supported(Value),
    Unsupported(GeminiCliRequestEnvelopeUnsupportedReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeminiCliRequestEnvelopeUnsupportedReason {
    NonObjectBody,
    MissingContents,
    MissingUserPromptId,
    MissingModel,
}

pub fn classify_gemini_cli_v1internal_request_body(
    request_body: &Value,
) -> Result<(), GeminiCliRequestEnvelopeUnsupportedReason> {
    let Value::Object(map) = request_body else {
        return Err(GeminiCliRequestEnvelopeUnsupportedReason::NonObjectBody);
    };
    if !map.contains_key("contents") && existing_v1internal_request_object(map).is_none() {
        return Err(GeminiCliRequestEnvelopeUnsupportedReason::MissingContents);
    }

    Ok(())
}

pub fn build_gemini_cli_v1internal_request(
    auth: &GeminiCliRequestAuth,
    user_prompt_id: &str,
    model: &str,
    request_body: &Value,
) -> GeminiCliRequestEnvelopeSupport {
    if user_prompt_id.trim().is_empty() {
        return GeminiCliRequestEnvelopeSupport::Unsupported(
            GeminiCliRequestEnvelopeUnsupportedReason::MissingUserPromptId,
        );
    }
    if model.trim().is_empty() {
        return GeminiCliRequestEnvelopeSupport::Unsupported(
            GeminiCliRequestEnvelopeUnsupportedReason::MissingModel,
        );
    }
    if let Err(reason) = classify_gemini_cli_v1internal_request_body(request_body) {
        return GeminiCliRequestEnvelopeSupport::Unsupported(reason);
    }

    let Value::Object(source) = request_body else {
        return GeminiCliRequestEnvelopeSupport::Unsupported(
            GeminiCliRequestEnvelopeUnsupportedReason::NonObjectBody,
        );
    };

    let existing_request = existing_v1internal_request_object(source);
    let mut inner_request: Map<String, Value> =
        existing_request.cloned().unwrap_or_else(|| source.clone());
    sanitize_inner_request(&mut inner_request);
    maybe_insert_session_id(&mut inner_request, auth.session_id.as_deref());

    let project = non_empty_string_field(source, "project")
        .map(ToOwned::to_owned)
        .or_else(|| {
            auth.project_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        });
    let user_prompt_id = non_empty_string_field(source, "user_prompt_id")
        .or_else(|| non_empty_string_field(source, "userPromptId"))
        .unwrap_or(user_prompt_id)
        .trim()
        .to_string();

    let mut envelope = Map::new();
    envelope.insert("model".to_string(), Value::String(model.trim().to_string()));
    if let Some(project) = project {
        envelope.insert("project".to_string(), Value::String(project));
    }
    envelope.insert("user_prompt_id".to_string(), Value::String(user_prompt_id));
    envelope.insert("request".to_string(), Value::Object(inner_request));

    GeminiCliRequestEnvelopeSupport::Supported(Value::Object(envelope))
}

fn sanitize_inner_request(inner_request: &mut Map<String, Value>) {
    inner_request.remove("model");
    inner_request.remove("stream");
}

fn maybe_insert_session_id(inner_request: &mut Map<String, Value>, session_id: Option<&str>) {
    let Some(session_id) = session_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    if inner_request.contains_key("session_id") || inner_request.contains_key("sessionId") {
        return;
    }
    inner_request.insert(
        "session_id".to_string(),
        Value::String(session_id.to_string()),
    );
}

fn existing_v1internal_request_object(source: &Map<String, Value>) -> Option<&Map<String, Value>> {
    source
        .get("request")
        .and_then(Value::as_object)
        .filter(|request| request.contains_key("contents"))
}

fn non_empty_string_field<'a>(source: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    source
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        build_gemini_cli_v1internal_request, classify_gemini_cli_v1internal_request_body,
        GeminiCliRequestEnvelopeSupport,
    };
    use crate::gemini_cli::GeminiCliRequestAuth;

    fn sample_auth() -> GeminiCliRequestAuth {
        GeminiCliRequestAuth {
            project_id: Some("project-123".to_string()),
            session_id: Some("session-123".to_string()),
        }
    }

    #[test]
    fn wraps_generate_content_body_in_gemini_cli_v1internal_envelope() {
        let request_body = json!({
            "model": "client-model",
            "contents": [
                {"role": "user", "parts": [{"text": "hello"}]}
            ],
            "stream": true,
            "generationConfig": {"temperature": 0.2}
        });

        assert_eq!(
            classify_gemini_cli_v1internal_request_body(&request_body),
            Ok(())
        );
        assert_eq!(
            build_gemini_cli_v1internal_request(
                &sample_auth(),
                "trace-123",
                "gemini-2.5-pro",
                &request_body,
            ),
            GeminiCliRequestEnvelopeSupport::Supported(json!({
                "model": "gemini-2.5-pro",
                "project": "project-123",
                "user_prompt_id": "trace-123",
                "request": {
                    "contents": [
                        {"role": "user", "parts": [{"text": "hello"}]}
                    ],
                    "generationConfig": {"temperature": 0.2},
                    "session_id": "session-123"
                }
            }))
        );
    }

    #[test]
    fn preserves_existing_v1internal_request_shape_without_antigravity_fields() {
        let request_body = json!({
            "model": "old-model",
            "project": "project-from-body",
            "user_prompt_id": "prompt-from-body",
            "request": {
                "model": "nested-client-model",
                "contents": [
                    {"role": "user", "parts": [{"text": "hello"}]}
                ],
                "stream": false,
                "labels": {"source": "test"}
            },
            "userAgent": "antigravity",
            "requestType": "agent"
        });

        assert_eq!(
            build_gemini_cli_v1internal_request(
                &sample_auth(),
                "trace-123",
                "gemini-2.5-pro",
                &request_body,
            ),
            GeminiCliRequestEnvelopeSupport::Supported(json!({
                "model": "gemini-2.5-pro",
                "project": "project-from-body",
                "user_prompt_id": "prompt-from-body",
                "request": {
                    "contents": [
                        {"role": "user", "parts": [{"text": "hello"}]}
                    ],
                    "labels": {"source": "test"},
                    "session_id": "session-123"
                }
            }))
        );
    }

    #[test]
    fn omits_optional_project_and_session_when_metadata_is_absent() {
        let request_body = json!({
            "contents": [
                {"role": "user", "parts": [{"text": "hello"}]}
            ]
        });

        assert_eq!(
            build_gemini_cli_v1internal_request(
                &GeminiCliRequestAuth::default(),
                "trace-123",
                "gemini-2.5-pro",
                &request_body,
            ),
            GeminiCliRequestEnvelopeSupport::Supported(json!({
                "model": "gemini-2.5-pro",
                "user_prompt_id": "trace-123",
                "request": {
                    "contents": [
                        {"role": "user", "parts": [{"text": "hello"}]}
                    ]
                }
            }))
        );
    }
}
