use std::collections::BTreeMap;

use serde_json::{json, Value};

pub use super::super::rules::{
    apply_local_body_rules_with_request_headers, apply_local_header_rules_with_request_headers,
    body_rules_are_locally_supported, header_rules_are_locally_supported,
};
use super::super::should_skip_upstream_passthrough_header;
use super::converter::convert_claude_messages_to_conversation_state;
use super::credentials::KiroAuthConfig;
use super::headers::build_generate_assistant_headers;

pub fn supports_local_kiro_request_shape(
    header_rules: Option<&Value>,
    body_rules: Option<&Value>,
) -> bool {
    header_rules_are_locally_supported(header_rules) && body_rules_are_locally_supported(body_rules)
}

pub fn build_kiro_provider_request_body(
    body_json: &Value,
    mapped_model: &str,
    auth_config: &KiroAuthConfig,
    body_rules: Option<&Value>,
    request_headers: Option<&http::HeaderMap>,
) -> Option<Value> {
    let conversation_state =
        convert_claude_messages_to_conversation_state(body_json, mapped_model)?;
    let mut provider_request_body = json!({
        "conversationState": conversation_state
    });

    let mut inference_config = serde_json::Map::new();
    if let Some(max_tokens) = body_json
        .get("max_tokens")
        .and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_u64().map(|value| value as i64))
        })
        .filter(|value| *value > 0)
    {
        inference_config.insert("maxTokens".to_string(), Value::from(max_tokens));
    }
    if let Some(temperature) = body_json
        .get("temperature")
        .and_then(Value::as_f64)
        .filter(|value| *value >= 0.0)
    {
        inference_config.insert("temperature".to_string(), Value::from(temperature));
    }
    if let Some(top_p) = body_json
        .get("top_p")
        .and_then(Value::as_f64)
        .filter(|value| *value > 0.0)
    {
        inference_config.insert("topP".to_string(), Value::from(top_p));
    }
    if !inference_config.is_empty() {
        provider_request_body.as_object_mut()?.insert(
            "inferenceConfig".to_string(),
            Value::Object(inference_config),
        );
    }

    if let Some(profile_arn) = auth_config.profile_arn_for_payload() {
        provider_request_body.as_object_mut()?.insert(
            "profileArn".to_string(),
            Value::String(profile_arn.to_string()),
        );
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

#[derive(Clone, Copy, Debug)]
pub struct KiroProviderHeadersInput<'a> {
    pub headers: &'a http::HeaderMap,
    pub provider_request_body: &'a Value,
    pub original_request_body: &'a Value,
    pub header_rules: Option<&'a Value>,
    pub auth_header: &'a str,
    pub auth_value: &'a str,
    pub auth_config: &'a KiroAuthConfig,
    pub machine_id: &'a str,
}

pub fn build_kiro_provider_headers(
    input: KiroProviderHeadersInput<'_>,
) -> Option<BTreeMap<String, String>> {
    let KiroProviderHeadersInput {
        headers,
        provider_request_body,
        original_request_body,
        header_rules,
        auth_header,
        auth_value,
        auth_config,
        machine_id,
    } = input;

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
        if value.is_empty() {
            continue;
        }
        out.insert(key, value.to_string());
    }

    if !apply_local_header_rules_with_request_headers(
        &mut out,
        header_rules,
        &[auth_header, "content-type"],
        provider_request_body,
        Some(original_request_body),
        Some(headers),
    ) {
        return None;
    }

    for (key, value) in build_generate_assistant_headers(auth_config, machine_id) {
        out.insert(key, value);
    }
    out.insert(
        auth_header.trim().to_ascii_lowercase(),
        auth_value.trim().to_string(),
    );
    out.entry("content-type".to_string())
        .or_insert_with(|| "application/json".to_string());
    out.remove("content-length");
    Some(out)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::credentials::KiroAuthConfig;
    use super::{
        build_kiro_provider_headers, build_kiro_provider_request_body,
        supports_local_kiro_request_shape, KiroProviderHeadersInput,
    };

    #[test]
    fn supports_empty_local_request_shape() {
        assert!(supports_local_kiro_request_shape(None, None));
    }

    #[test]
    fn rejects_unsupported_rule_shape() {
        assert!(!supports_local_kiro_request_shape(
            Some(&json!({"action":"set"})),
            None
        ));
    }

    #[test]
    fn supports_simple_header_and_body_rules() {
        assert!(supports_local_kiro_request_shape(
            Some(&json!([{"action":"set","key":"x-provider-extra","value":"1"}])),
            Some(&json!([{"action":"set","path":"debugTag","value":true}]))
        ));
    }

    #[test]
    fn wraps_claude_request_into_kiro_payload_before_body_rules() {
        let auth_config = KiroAuthConfig {
            auth_method: None,
            refresh_token: Some("r".repeat(128)),
            expires_at: None,
            profile_arn: Some("arn:aws:bedrock:demo".to_string()),
            region: None,
            auth_region: None,
            api_region: Some("us-east-1".to_string()),
            client_id: None,
            client_secret: None,
            machine_id: Some("123e4567-e89b-12d3-a456-426614174000".to_string()),
            kiro_version: None,
            system_version: None,
            node_version: None,
            access_token: Some("cached-token".to_string()),
        };

        let payload = build_kiro_provider_request_body(
            &json!({
                "messages": [{"role":"user","content":"hello"}],
                "max_tokens": 64
            }),
            "claude-sonnet-4-upstream",
            &auth_config,
            Some(&json!([
                {"action":"set","path":"debugTag","value":"kiro-local"}
            ])),
            None,
        )
        .expect("payload should build");

        assert!(payload.get("conversationState").is_some());
        assert_eq!(
            payload
                .get("inferenceConfig")
                .and_then(|value| value.get("maxTokens")),
            Some(&json!(64))
        );
        assert_eq!(
            payload.get("profileArn"),
            Some(&json!("arn:aws:bedrock:demo"))
        );
        assert_eq!(payload.get("debugTag"), Some(&json!("kiro-local")));
    }

    #[test]
    fn applies_header_rules_before_kiro_extra_headers() {
        let auth_config = KiroAuthConfig {
            auth_method: None,
            refresh_token: Some("r".repeat(128)),
            expires_at: None,
            profile_arn: None,
            region: None,
            auth_region: None,
            api_region: Some("us-east-1".to_string()),
            client_id: None,
            client_secret: None,
            machine_id: None,
            kiro_version: None,
            system_version: None,
            node_version: None,
            access_token: Some("cached-token".to_string()),
        };
        let headers = build_kiro_provider_headers(KiroProviderHeadersInput {
            headers: &http::HeaderMap::new(),
            provider_request_body: &json!({"conversationState": {}}),
            original_request_body: &json!({"messages": []}),
            header_rules: Some(&json!([
                {"action":"set","key":"accept","value":"text/plain"},
                {"action":"set","key":"x-endpoint-tag","value":"kiro-local"}
            ])),
            auth_header: "authorization",
            auth_value: "Bearer cached-token",
            auth_config: &auth_config,
            machine_id: "machine-123",
        })
        .expect("headers should build");

        assert_eq!(
            headers.get("accept").map(String::as_str),
            Some("application/vnd.amazon.eventstream")
        );
        assert_eq!(
            headers.get("authorization").map(String::as_str),
            Some("Bearer cached-token")
        );
        assert_eq!(
            headers.get("x-endpoint-tag").map(String::as_str),
            Some("kiro-local")
        );
    }

    #[test]
    fn omits_profile_arn_for_idc_auth() {
        let auth_config = KiroAuthConfig {
            auth_method: Some("identity_center".to_string()),
            refresh_token: Some("r".repeat(128)),
            expires_at: None,
            profile_arn: Some("arn:aws:bedrock:demo".to_string()),
            region: None,
            auth_region: None,
            api_region: Some("us-east-1".to_string()),
            client_id: Some("cid".to_string()),
            client_secret: Some("secret".to_string()),
            machine_id: None,
            kiro_version: None,
            system_version: None,
            node_version: None,
            access_token: Some("cached-token".to_string()),
        };

        let payload = build_kiro_provider_request_body(
            &json!({
                "messages": [{"role":"user","content":"hello"}]
            }),
            "claude-sonnet-4-upstream",
            &auth_config,
            None,
            None,
        )
        .expect("payload should build");

        assert!(payload.get("profileArn").is_none());
    }
}
