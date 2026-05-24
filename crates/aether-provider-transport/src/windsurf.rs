use std::collections::BTreeMap;

use serde_json::{json, Value};
use uuid::Uuid;

use crate::rules::{
    apply_local_body_rules_with_request_headers, apply_local_header_rules_with_request_headers,
    body_rules_are_locally_supported, header_rules_are_locally_supported,
};
use crate::snapshot::GatewayProviderTransportSnapshot;
use crate::url::build_passthrough_path_url;
use crate::{
    resolve_transport_profile, should_skip_upstream_passthrough_header,
    supports_local_oauth_request_auth_resolution, transport_profile_is_configured,
    transport_proxy_is_locally_supported,
};

pub mod cascade;
pub mod models;
pub mod proto;

pub const PROVIDER_TYPE: &str = "windsurf";
pub const WINDSURF_ENVELOPE_NAME: &str = "windsurf:GetChatMessage";
pub const GET_CHAT_MESSAGE_PATH: &str = "/exa.api_server_pb.ApiServerService/GetChatMessage";
const DEFAULT_IDE_VERSION: &str = "1.9600.41";
const PLACEHOLDER_API_KEY: &str = "__placeholder__";

pub fn is_windsurf_provider_transport(transport: &GatewayProviderTransportSnapshot) -> bool {
    transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case(PROVIDER_TYPE)
}

pub fn local_windsurf_request_transport_unsupported_reason_with_network(
    transport: &GatewayProviderTransportSnapshot,
) -> Option<&'static str> {
    if !transport.provider.is_active {
        return Some("provider_inactive");
    }
    if !transport.endpoint.is_active {
        return Some("endpoint_inactive");
    }
    if !transport.key.is_active {
        return Some("key_inactive");
    }
    if !is_windsurf_provider_transport(transport) {
        return Some("transport_provider_type_unsupported");
    }
    if !transport
        .endpoint
        .api_format
        .trim()
        .eq_ignore_ascii_case("openai:chat")
    {
        return Some("transport_api_format_mismatch");
    }
    if !header_rules_are_locally_supported(transport.endpoint.header_rules.as_ref()) {
        return Some("transport_header_rules_unsupported");
    }
    if !body_rules_are_locally_supported(transport.endpoint.body_rules.as_ref()) {
        return Some("transport_body_rules_unsupported");
    }
    if transport.key.decrypted_auth_config.is_some()
        && !supports_local_oauth_request_auth_resolution(transport)
        && !supports_local_windsurf_request_auth_resolution(transport)
    {
        return Some("transport_oauth_resolution_unsupported");
    }
    if !transport_proxy_is_locally_supported(transport) {
        return Some("transport_proxy_unsupported");
    }
    if transport_profile_is_configured(transport) && resolve_transport_profile(transport).is_none()
    {
        return Some("transport_profile_unsupported");
    }
    None
}

pub fn supports_local_windsurf_request_auth_resolution(
    transport: &GatewayProviderTransportSnapshot,
) -> bool {
    resolve_windsurf_cascade_auth(transport).is_some()
}

pub fn resolve_windsurf_cascade_auth(
    transport: &GatewayProviderTransportSnapshot,
) -> Option<(String, String)> {
    if !is_windsurf_provider_transport(transport) {
        return None;
    }
    let auth_type = transport.key.auth_type.trim().to_ascii_lowercase();
    if !matches!(auth_type.as_str(), "oauth" | "api_key" | "bearer") {
        return None;
    }
    let secret = transport.key.decrypted_api_key.trim();
    if secret.is_empty() || secret == PLACEHOLDER_API_KEY {
        return None;
    }
    Some(("authorization".to_string(), format!("Bearer {secret}")))
}

pub fn build_windsurf_cascade_upstream_url(
    upstream_base_url: &str,
    query: Option<&str>,
) -> Option<String> {
    build_passthrough_path_url(upstream_base_url, GET_CHAT_MESSAGE_PATH, query, &[])
}

pub fn build_windsurf_cascade_request_body(
    body_json: &Value,
    mapped_model: &str,
    auth_value: &str,
    body_rules: Option<&Value>,
    request_headers: Option<&http::HeaderMap>,
    upstream_is_stream: bool,
) -> Option<Value> {
    let mapped_model = mapped_model.trim();
    if mapped_model.is_empty() {
        return None;
    }
    let messages = body_json.get("messages")?.as_array()?.clone();
    if messages.is_empty() {
        return None;
    }
    let conversation_id =
        extract_conversation_id(body_json).unwrap_or_else(|| Uuid::new_v4().to_string());
    let message_text =
        latest_message_snapshot_text(&messages).unwrap_or_else(|| "Continue.".to_string());
    let mut provider_request_body = json!({
        "metadata": windsurf_metadata_from_auth(auth_value),
        "model": mapped_model,
        "modelName": mapped_model,
        "stream": upstream_is_stream,
        "conversationId": conversation_id,
        "message": message_text,
        "messages": messages,
    });

    if let Some(max_tokens) = body_json
        .get("max_tokens")
        .or_else(|| body_json.get("maxTokens"))
    {
        provider_request_body
            .as_object_mut()?
            .insert("maxTokens".to_string(), max_tokens.clone());
    }
    if let Some(temperature) = body_json.get("temperature") {
        provider_request_body
            .as_object_mut()?
            .insert("temperature".to_string(), temperature.clone());
    }
    if let Some(top_p) = body_json.get("top_p").or_else(|| body_json.get("topP")) {
        provider_request_body
            .as_object_mut()?
            .insert("topP".to_string(), top_p.clone());
    }
    for field in [
        "tools",
        "tool_choice",
        "toolChoice",
        "parallel_tool_calls",
        "response_format",
    ] {
        if let Some(value) = body_json.get(field) {
            provider_request_body
                .as_object_mut()?
                .insert(field.to_string(), value.clone());
        }
    }

    if !apply_local_body_rules_with_request_headers(
        &mut provider_request_body,
        body_rules,
        Some(body_json),
        request_headers,
    ) {
        return None;
    }
    Some(provider_request_body)
}

pub fn build_windsurf_cascade_headers(
    headers: &http::HeaderMap,
    provider_request_body: &Value,
    original_request_body: &Value,
    header_rules: Option<&Value>,
    auth_header: &str,
    auth_value: &str,
    _upstream_is_stream: bool,
) -> Option<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for (name, value) in headers {
        let Ok(value) = value.to_str() else {
            continue;
        };
        let key = name.as_str().to_ascii_lowercase();
        if should_skip_upstream_passthrough_header(&key) {
            continue;
        }
        let value = value.trim();
        if !value.is_empty() {
            out.insert(key, value.to_string());
        }
    }

    let auth_header = auth_header.trim().to_ascii_lowercase();
    if !apply_local_header_rules_with_request_headers(
        &mut out,
        header_rules,
        &[
            auth_header.as_str(),
            "content-type",
            "connect-protocol-version",
        ],
        provider_request_body,
        Some(original_request_body),
        Some(headers),
    ) {
        return None;
    }

    out.insert(
        "content-type".to_string(),
        "application/connect+json".to_string(),
    );
    out.insert("connect-protocol-version".to_string(), "1".to_string());
    out.insert(
        "user-agent".to_string(),
        format!("windsurf/{DEFAULT_IDE_VERSION}"),
    );
    out.insert("accept".to_string(), "application/connect+json".to_string());
    if !auth_header.is_empty() {
        out.insert(auth_header, auth_value.trim().to_string());
    }
    out.remove("content-length");
    Some(out)
}

fn windsurf_metadata_from_auth(auth_value: &str) -> Value {
    json!({
        "apiKey": auth_secret_from_header_value(auth_value),
        "ideName": "windsurf",
        "ideVersion": DEFAULT_IDE_VERSION,
        "extensionName": "windsurf",
        "extensionVersion": DEFAULT_IDE_VERSION,
        "locale": "en",
    })
}

fn auth_secret_from_header_value(auth_value: &str) -> String {
    let value = auth_value.trim();
    value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))
        .unwrap_or(value)
        .trim()
        .to_string()
}

fn extract_conversation_id(body_json: &Value) -> Option<String> {
    let object = body_json.as_object()?;
    string_value(object.get("conversation_id"))
        .or_else(|| string_value(object.get("conversationId")))
        .or_else(|| string_value(object.get("session_id")))
        .or_else(|| string_value(object.get("sessionId")))
        .or_else(|| {
            object
                .get("metadata")
                .and_then(Value::as_object)
                .and_then(|metadata| {
                    string_value(metadata.get("conversation_id"))
                        .or_else(|| string_value(metadata.get("conversationId")))
                        .or_else(|| string_value(metadata.get("session_id")))
                        .or_else(|| string_value(metadata.get("sessionId")))
                })
        })
}

fn string_value(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn latest_message_snapshot_text(messages: &[Value]) -> Option<String> {
    messages
        .iter()
        .rev()
        .filter_map(Value::as_object)
        .find_map(|message| {
            let role = message.get("role").and_then(Value::as_str)?;
            match role {
                "user" => openai_content_to_text(message.get("content"))
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty()),
                "tool" => {
                    let content = openai_content_to_text(message.get("content"))
                        .map(|value| value.trim().to_string())
                        .filter(|value| !value.is_empty())?;
                    let tool_call_id = message
                        .get("tool_call_id")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    Some(format!(
                        "<tool_result tool_call_id=\"{}\">\n{content}\n</tool_result>",
                        escape_xml_attr(tool_call_id)
                    ))
                }
                "assistant" => openai_content_to_text(message.get("content"))
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty()),
                _ => None,
            }
        })
}

fn openai_content_to_text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => {
            let parts = items
                .iter()
                .filter_map(|item| {
                    item.as_object()
                        .and_then(|object| object.get("text"))
                        .and_then(Value::as_str)
                })
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>();
            (!parts.is_empty()).then(|| parts.join("\n"))
        }
        _ => None,
    }
}

fn escape_xml_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use http::HeaderMap;
    use serde_json::json;

    use crate::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };

    use super::{
        build_windsurf_cascade_headers, build_windsurf_cascade_request_body,
        build_windsurf_cascade_upstream_url,
        local_windsurf_request_transport_unsupported_reason_with_network,
        resolve_windsurf_cascade_auth, GET_CHAT_MESSAGE_PATH,
    };

    fn sample_windsurf_transport(auth_type: &str) -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-windsurf".to_string(),
                name: "Windsurf".to_string(),
                provider_type: "windsurf".to_string(),
                website: None,
                is_active: true,
                keep_priority_on_conversion: false,
                enable_format_conversion: true,
                concurrent_limit: None,
                max_retries: None,
                proxy: None,
                request_timeout_secs: None,
                stream_first_byte_timeout_secs: None,
                config: None,
            },
            endpoint: GatewayProviderTransportEndpoint {
                id: "endpoint-windsurf-chat".to_string(),
                provider_id: "provider-windsurf".to_string(),
                api_format: "openai:chat".to_string(),
                api_family: Some("openai".to_string()),
                endpoint_kind: Some("chat".to_string()),
                is_active: true,
                base_url: "https://server.codeium.com".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: None,
                config: None,
                format_acceptance_config: None,
                proxy: None,
            },
            key: GatewayProviderTransportKey {
                id: "key-windsurf".to_string(),
                provider_id: "provider-windsurf".to_string(),
                name: "windsurf@example.com".to_string(),
                auth_type: auth_type.to_string(),
                is_active: true,
                api_formats: None,
                auth_type_by_format: None,
                allow_auth_channel_mismatch_formats: None,
                allowed_models: None,
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: None,
                expires_at_unix_secs: None,
                proxy: None,
                fingerprint: None,
                decrypted_api_key: "devin-session-token$abc".to_string(),
                decrypted_auth_config: Some(r#"{"provider_type":"windsurf"}"#.to_string()),
            },
        }
    }

    #[test]
    fn builds_windsurf_cascade_url() {
        assert_eq!(
            build_windsurf_cascade_upstream_url("https://server.codeium.com", Some("debug=1"))
                .as_deref(),
            Some(
                "https://server.codeium.com/exa.api_server_pb.ApiServerService/GetChatMessage?debug=1"
            )
        );
        assert!(GET_CHAT_MESSAGE_PATH.ends_with("/GetChatMessage"));
    }

    #[test]
    fn builds_cascade_request_body_with_metadata_and_messages() {
        let body = build_windsurf_cascade_request_body(
            &json!({
                "model": "gpt-5",
                "conversation_id": "conv-1",
                "messages": [
                    {"role": "system", "content": "brief"},
                    {"role": "user", "content": [{"type": "text", "text": "hello"}]}
                ],
                "max_tokens": 128
            }),
            "windsurf-model",
            "Bearer devin-session-token$abc",
            None,
            None,
            true,
        )
        .expect("body should build");

        assert_eq!(body["metadata"]["apiKey"], json!("devin-session-token$abc"));
        assert_eq!(body["modelName"], json!("windsurf-model"));
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["conversationId"], json!("conv-1"));
        assert_eq!(body["message"], json!("hello"));
        assert_eq!(body["maxTokens"], json!(128));
    }

    #[test]
    fn preserves_openai_tool_fields_for_native_windsurf_runtime() {
        let body = build_windsurf_cascade_request_body(
            &json!({
                "model": "gpt-5-5-low",
                "messages": [
                    {"role": "user", "content": "read Cargo.toml"}
                ],
                "tools": [{
                    "type": "function",
                    "function": {
                        "name": "Read",
                        "parameters": {
                            "type": "object",
                            "properties": {"file_path": {"type": "string"}}
                        }
                    }
                }],
                "tool_choice": "required",
                "parallel_tool_calls": false,
                "response_format": {"type": "json_object"}
            }),
            "gpt-5-5-low",
            "Bearer devin-session-token$abc",
            None,
            None,
            false,
        )
        .expect("body should build");

        assert_eq!(body["tools"][0]["function"]["name"], json!("Read"));
        assert_eq!(body["tool_choice"], json!("required"));
        assert_eq!(body["parallel_tool_calls"], json!(false));
        assert_eq!(body["response_format"]["type"], json!("json_object"));
    }

    #[test]
    fn builds_cascade_request_body_message_snapshot_from_latest_tool_result() {
        let body = build_windsurf_cascade_request_body(
            &json!({
                "model": "gpt-5-5-low",
                "messages": [
                    {"role": "user", "content": "read Cargo.toml"},
                    {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "call_1",
                            "type": "function",
                            "function": {"name": "Read", "arguments": "{\"file_path\":\"Cargo.toml\"}"}
                        }]
                    },
                    {"role": "tool", "tool_call_id": "call_1", "content": "workspace Cargo.toml content"}
                ]
            }),
            "gpt-5-5-low",
            "Bearer devin-session-token$abc",
            None,
            None,
            false,
        )
        .expect("body should build");

        assert!(body["message"]
            .as_str()
            .expect("message should be a string")
            .contains(r#"<tool_result tool_call_id="call_1">"#));
        assert!(body["message"]
            .as_str()
            .expect("message should be a string")
            .contains("workspace Cargo.toml content"));
    }

    #[test]
    fn builds_cascade_headers_with_connect_protocol_and_auth() {
        let headers = build_windsurf_cascade_headers(
            &HeaderMap::new(),
            &json!({"metadata": {"apiKey": "secret"}}),
            &json!({"messages": []}),
            None,
            "authorization",
            "Bearer secret",
            false,
        )
        .expect("headers should build");

        assert_eq!(
            headers.get("connect-protocol-version").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            headers.get("authorization").map(String::as_str),
            Some("Bearer secret")
        );
        assert_eq!(
            headers.get("content-type").map(String::as_str),
            Some("application/connect+json")
        );
        assert_eq!(
            headers.get("accept").map(String::as_str),
            Some("application/connect+json")
        );
    }

    #[test]
    fn oauth_windsurf_transport_resolves_direct_bearer_auth() {
        let transport = sample_windsurf_transport("oauth");

        assert_eq!(
            local_windsurf_request_transport_unsupported_reason_with_network(&transport),
            None
        );
        assert_eq!(
            resolve_windsurf_cascade_auth(&transport),
            Some((
                "authorization".to_string(),
                "Bearer devin-session-token$abc".to_string()
            ))
        );
    }
}
