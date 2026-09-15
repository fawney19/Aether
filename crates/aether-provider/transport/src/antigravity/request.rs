use serde_json::{Map, Value};

use super::auth::{AntigravityRequestAuth, ANTIGRAVITY_REQUEST_USER_AGENT};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AntigravityEnvelopeRequestType {
    Agent,
    Checkpoint,
    EndpointTest,
}

impl AntigravityEnvelopeRequestType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Checkpoint => "checkpoint",
            Self::EndpointTest => "endpoint_test",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AntigravityRequestEnvelopeSupport {
    Supported(Value),
    Unsupported(AntigravityRequestEnvelopeUnsupportedReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AntigravityRequestEnvelopeUnsupportedReason {
    NonObjectBody,
    MissingContents,
    MissingRequestId,
    MissingModel,
}

pub fn classify_antigravity_safe_request_body(
    request_body: &Value,
) -> Result<(), AntigravityRequestEnvelopeUnsupportedReason> {
    let Value::Object(map) = request_body else {
        return Err(AntigravityRequestEnvelopeUnsupportedReason::NonObjectBody);
    };
    if !map.contains_key("contents") && existing_v1internal_request_object(map).is_none() {
        return Err(AntigravityRequestEnvelopeUnsupportedReason::MissingContents);
    }

    Ok(())
}

pub fn build_antigravity_safe_v1internal_request(
    auth: &AntigravityRequestAuth,
    request_id: &str,
    model: &str,
    request_body: &Value,
    request_type: AntigravityEnvelopeRequestType,
) -> AntigravityRequestEnvelopeSupport {
    if request_id.trim().is_empty() {
        return AntigravityRequestEnvelopeSupport::Unsupported(
            AntigravityRequestEnvelopeUnsupportedReason::MissingRequestId,
        );
    }
    if model.trim().is_empty() {
        return AntigravityRequestEnvelopeSupport::Unsupported(
            AntigravityRequestEnvelopeUnsupportedReason::MissingModel,
        );
    }
    if let Err(reason) = classify_antigravity_safe_request_body(request_body) {
        return AntigravityRequestEnvelopeSupport::Unsupported(reason);
    }

    let Value::Object(source) = request_body else {
        return AntigravityRequestEnvelopeSupport::Unsupported(
            AntigravityRequestEnvelopeUnsupportedReason::NonObjectBody,
        );
    };

    if let Some(existing_request) = existing_v1internal_request_object(source) {
        let mut inner_request: Map<String, Value> = existing_request.clone();
        inner_request.remove("model");
        inner_request.remove("safetySettings");
        inner_request.remove("safety_settings");
        normalize_antigravity_builtin_tool_names(&mut inner_request);
        normalize_antigravity_function_declaration_parameters(&mut inner_request);
        let request_id = non_empty_string_field(source, "requestId").unwrap_or(request_id);
        let user_agent =
            non_empty_string_field(source, "userAgent").unwrap_or(ANTIGRAVITY_REQUEST_USER_AGENT);
        let existing_request_type = existing_v1internal_request_type(source);

        let mut envelope = serde_json::json!({
            "project": auth.project_id,
            "requestId": request_id,
            "request": Value::Object(inner_request),
            "model": model,
            "userAgent": user_agent,
        });
        if let Some(existing_request_type) = existing_request_type {
            envelope["requestType"] = Value::String(existing_request_type.to_string());
        } else if request_type != AntigravityEnvelopeRequestType::Agent {
            envelope["requestType"] = Value::String(request_type.as_str().to_string());
        }
        return AntigravityRequestEnvelopeSupport::Supported(envelope);
    }

    let mut inner_request: Map<String, Value> = source.clone();
    inner_request.remove("model");
    inner_request.remove("safetySettings");
    inner_request.remove("safety_settings");
    normalize_antigravity_builtin_tool_names(&mut inner_request);
    normalize_antigravity_function_declaration_parameters(&mut inner_request);

    let mut envelope = serde_json::json!({
        "project": auth.project_id,
        "requestId": request_id,
        "request": Value::Object(inner_request),
        "model": model,
        "userAgent": ANTIGRAVITY_REQUEST_USER_AGENT,
    });
    if request_type != AntigravityEnvelopeRequestType::Agent {
        envelope["requestType"] = Value::String(request_type.as_str().to_string());
    }
    AntigravityRequestEnvelopeSupport::Supported(envelope)
}

/// Antigravity's private v1internal Gemini surface still uses the legacy
/// `googleSearchRetrieval` spelling. The public Gemini converter emits the
/// newer `googleSearch` spelling, which the private backend rejects when it is
/// combined with function declarations. Normalize only at this transport
/// boundary so public Gemini requests retain their native shape.
fn normalize_antigravity_builtin_tool_names(request: &mut Map<String, Value>) {
    let Some(tools) = request.get_mut("tools").and_then(Value::as_array_mut) else {
        return;
    };

    for tool in tools {
        let Some(tool_object) = tool.as_object_mut() else {
            continue;
        };

        if let Some(payload) = tool_object.remove("googleSearch") {
            tool_object
                .entry("googleSearchRetrieval".to_string())
                .or_insert(payload);
        }
        if let Some(payload) = tool_object.remove("google_search") {
            tool_object
                .entry("googleSearchRetrieval".to_string())
                .or_insert(payload);
        }
    }
}

fn normalize_antigravity_function_declaration_parameters(request: &mut Map<String, Value>) {
    let Some(tools) = request.get_mut("tools").and_then(Value::as_array_mut) else {
        return;
    };

    for tool in tools {
        let Some(tool_object) = tool.as_object_mut() else {
            continue;
        };
        for key in ["functionDeclarations", "function_declarations"] {
            let Some(declarations) = tool_object.get_mut(key).and_then(Value::as_array_mut) else {
                continue;
            };
            for declaration in declarations {
                let Some(declaration_object) = declaration.as_object_mut() else {
                    continue;
                };
                if let Some(parameters) = declaration_object.remove("parametersJsonSchema") {
                    declaration_object
                        .entry("parameters".to_string())
                        .or_insert(parameters);
                }
                if let Some(parameters) = declaration_object.remove("parameters_json_schema") {
                    declaration_object
                        .entry("parameters".to_string())
                        .or_insert(parameters);
                }
            }
        }
    }
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

fn existing_v1internal_request_type(source: &Map<String, Value>) -> Option<&str> {
    match non_empty_string_field(source, "requestType")? {
        "agent" => Some("agent"),
        "checkpoint" => Some("checkpoint"),
        "endpoint_test" => Some("endpoint_test"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        build_antigravity_safe_v1internal_request, classify_antigravity_safe_request_body,
        AntigravityEnvelopeRequestType, AntigravityRequestAuth, AntigravityRequestEnvelopeSupport,
    };
    use crate::antigravity::ANTIGRAVITY_REQUEST_USER_AGENT;

    fn sample_auth() -> AntigravityRequestAuth {
        AntigravityRequestAuth {
            project_id: "project-ant-123".to_string(),
            client_version: None,
            session_id: None,
        }
    }

    #[test]
    fn real_agent_request_preserves_antigravity_agent_fields() {
        let request_body = json!({
            "model": "client-side-model-should-not-be-nested",
            "contents": [
                {
                    "role": "user",
                    "parts": [
                        { "text": "Reply with OK only." }
                    ]
                }
            ],
            "systemInstruction": {
                "role": "user",
                "parts": [
                    { "text": "Antigravity agent system prompt" }
                ]
            },
            "generationConfig": {
                "maxOutputTokens": 8192,
                "thinkingConfig": {
                    "includeThoughts": true,
                    "thinkingBudget": 4000
                }
            },
            "toolConfig": {
                "includeServerSideToolInvocations": true,
                "functionCallingConfig": {
                    "mode": "VALIDATED"
                }
            },
            "tools": [
                {
                    "googleSearch": {}
                },
                {
                    "functionDeclarations": [
                        {
                            "name": "run_command",
                            "description": "Run a command",
                            "parameters": {
                                "type": "object",
                                "properties": {
                                    "cmd": { "type": "string" }
                                },
                                "required": ["cmd"]
                            }
                        }
                    ]
                }
            ],
            "labels": {
                "trajectory_id": "trajectory-123",
                "used_claude": "false"
            },
            "sessionId": "session-ant-123",
            "safetySettings": [
                { "category": "HARM_CATEGORY_UNSPECIFIED" }
            ]
        });

        assert_eq!(
            classify_antigravity_safe_request_body(&request_body),
            Ok(())
        );

        let envelope = match build_antigravity_safe_v1internal_request(
            &sample_auth(),
            "request-ant-agent-123",
            "gemini-3.5-flash-low",
            &request_body,
            AntigravityEnvelopeRequestType::Agent,
        ) {
            AntigravityRequestEnvelopeSupport::Supported(envelope) => envelope,
            AntigravityRequestEnvelopeSupport::Unsupported(reason) => {
                panic!("real agent envelope should be supported: {reason:?}")
            }
        };

        assert_eq!(envelope["project"], "project-ant-123");
        assert_eq!(envelope["requestId"], "request-ant-agent-123");
        assert_eq!(envelope["model"], "gemini-3.5-flash-low");
        assert_eq!(envelope["userAgent"], ANTIGRAVITY_REQUEST_USER_AGENT);
        assert!(envelope.get("requestType").is_none());
        assert!(envelope["request"].get("model").is_none());
        assert!(envelope["request"].get("safetySettings").is_none());
        assert_eq!(
            envelope["request"]["systemInstruction"]["parts"][0]["text"],
            "Antigravity agent system prompt"
        );
        assert_eq!(
            envelope["request"]["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            4000
        );
        assert_eq!(
            envelope["request"]["toolConfig"]["functionCallingConfig"]["mode"],
            "VALIDATED"
        );
        assert_eq!(
            envelope["request"]["toolConfig"]["includeServerSideToolInvocations"],
            true
        );
        assert!(envelope["request"]["toolConfig"]
            .get("include_server_side_tool_invocations")
            .is_none());
        assert!(envelope["request"]["tools"][0]
            .get("googleSearch")
            .is_none());
        assert_eq!(
            envelope["request"]["tools"][0]["googleSearchRetrieval"],
            json!({})
        );
        assert_eq!(
            envelope["request"]["tools"][1]["functionDeclarations"][0]["name"],
            "run_command"
        );
        assert_eq!(
            envelope["request"]["tools"][1]["functionDeclarations"][0]["parameters"]["properties"]
                ["cmd"]["type"],
            "string"
        );
        assert!(envelope["request"]["tools"][1]["functionDeclarations"][0]
            .get("parametersJsonSchema")
            .is_none());
        assert_eq!(
            envelope["request"]["labels"]["trajectory_id"],
            "trajectory-123"
        );
        assert_eq!(envelope["request"]["sessionId"], "session-ant-123");
    }

    #[test]
    fn checkpoint_request_type_builds_checkpoint_envelope() {
        let request_body = json!({
            "contents": [
                {
                    "role": "user",
                    "parts": [
                        { "text": "checkpoint context" }
                    ]
                }
            ],
            "generationConfig": {
                "maxOutputTokens": 8192,
                "thinkingConfig": {
                    "includeThoughts": true,
                    "thinkingBudget": 4000
                }
            },
            "toolConfig": {
                "functionCallingConfig": {
                    "mode": "NONE"
                }
            }
        });

        let envelope = match build_antigravity_safe_v1internal_request(
            &sample_auth(),
            "request-ant-checkpoint-123",
            "gemini-3.5-flash-low",
            &request_body,
            AntigravityEnvelopeRequestType::Checkpoint,
        ) {
            AntigravityRequestEnvelopeSupport::Supported(envelope) => envelope,
            AntigravityRequestEnvelopeSupport::Unsupported(reason) => {
                panic!("checkpoint envelope should be supported: {reason:?}")
            }
        };

        assert_eq!(envelope["requestType"], "checkpoint");
        assert_eq!(
            envelope["request"]["toolConfig"]["functionCallingConfig"]["mode"],
            "NONE"
        );
    }

    #[test]
    fn existing_v1internal_envelope_is_not_double_wrapped() {
        let request_body = json!({
            "project": "client-side-project",
            "requestId": "client-request-id-123",
            "model": "gemini-3.5-flash-low",
            "userAgent": "antigravity",
            "requestType": "checkpoint",
            "request": {
                "contents": [
                    {
                        "role": "user",
                        "parts": [
                            { "text": "checkpoint context" }
                        ]
                    }
                ],
                "generationConfig": {
                    "thinkingConfig": {
                        "includeThoughts": true
                    }
                },
                "toolConfig": {
                    "functionCallingConfig": {
                        "mode": "NONE"
                    }
                },
                "tools": [{
                    "google_search": {
                        "dynamicRetrievalConfig": {
                            "mode": "MODE_UNSPECIFIED"
                        }
                    }
                }]
            }
        });

        assert_eq!(
            classify_antigravity_safe_request_body(&request_body),
            Ok(())
        );

        let envelope = match build_antigravity_safe_v1internal_request(
            &sample_auth(),
            "trace-request-id-should-not-overwrite-client-id",
            "mapped-antigravity-model",
            &request_body,
            AntigravityEnvelopeRequestType::Agent,
        ) {
            AntigravityRequestEnvelopeSupport::Supported(envelope) => envelope,
            AntigravityRequestEnvelopeSupport::Unsupported(reason) => {
                panic!("existing v1internal envelope should be supported: {reason:?}")
            }
        };

        assert_eq!(envelope["project"], "project-ant-123");
        assert_eq!(envelope["requestId"], "client-request-id-123");
        assert_eq!(envelope["model"], "mapped-antigravity-model");
        assert_eq!(envelope["userAgent"], "antigravity");
        assert_eq!(envelope["requestType"], "checkpoint");
        assert!(envelope["request"].get("request").is_none());
        assert_eq!(
            envelope["request"]["contents"][0]["parts"][0]["text"],
            "checkpoint context"
        );
        assert_eq!(
            envelope["request"]["toolConfig"]["functionCallingConfig"]["mode"],
            "NONE"
        );
        assert!(envelope["request"]["tools"][0]
            .get("google_search")
            .is_none());
        assert_eq!(
            envelope["request"]["tools"][0]["googleSearchRetrieval"],
            json!({
                "dynamicRetrievalConfig": {
                    "mode": "MODE_UNSPECIFIED"
                }
            })
        );
    }

    #[test]
    fn antigravity_envelope_normalizes_json_schema_parameter_spellings() {
        let request_body = json!({
            "contents": [{
                "role": "user",
                "parts": [{ "text": "hello" }]
            }],
            "tools": [{
                "function_declarations": [{
                    "name": "lookup",
                    "parametersJsonSchema": { "type": "object" }
                }, {
                    "name": "weather",
                    "parameters_json_schema": { "type": "object" }
                }]
            }]
        });

        let envelope = match build_antigravity_safe_v1internal_request(
            &sample_auth(),
            "request-ant-schema-123",
            "gemini-3.5-flash-low",
            &request_body,
            AntigravityEnvelopeRequestType::Agent,
        ) {
            AntigravityRequestEnvelopeSupport::Supported(envelope) => envelope,
            AntigravityRequestEnvelopeSupport::Unsupported(reason) => {
                panic!("schema envelope should be supported: {reason:?}")
            }
        };

        let declarations = &envelope["request"]["tools"][0]["function_declarations"];
        assert_eq!(declarations[0]["parameters"]["type"], "object");
        assert_eq!(declarations[1]["parameters"]["type"], "object");
        assert!(declarations[0].get("parametersJsonSchema").is_none());
        assert!(declarations[1].get("parameters_json_schema").is_none());
    }
}
