use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::auth::{build_passthrough_headers_with_auth, resolve_local_gemini_auth};
use crate::policy::local_gemini_transport_unsupported_reason_with_network;
use crate::rules::{apply_local_body_rules, apply_local_header_rules};
use crate::snapshot::GatewayProviderTransportSnapshot;
use crate::url::build_gemini_files_passthrough_url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeminiFilesRequestBodyError {
    BodyRulesUnsupportedForBinaryUpload,
    BodyRulesApplyFailed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GeminiFilesRequestBodyParts {
    pub provider_request_body: Option<Value>,
    pub provider_request_body_base64: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct GeminiFilesHeadersInput<'a> {
    pub headers: &'a http::HeaderMap,
    pub auth_header: &'a str,
    pub auth_value: &'a str,
    pub header_rules: Option<&'a Value>,
    pub provider_request_body: Option<&'a Value>,
    pub provider_request_body_base64: Option<&'a str>,
    pub original_request_body_json: &'a Value,
    pub original_body_is_empty: bool,
}

pub fn gemini_files_transport_unsupported_reason(
    transport: &GatewayProviderTransportSnapshot,
    api_format: &str,
) -> Option<&'static str> {
    local_gemini_transport_unsupported_reason_with_network(transport, api_format)
}

pub fn resolve_gemini_files_auth(
    transport: &GatewayProviderTransportSnapshot,
) -> Option<(String, String)> {
    resolve_local_gemini_auth(transport)
}

pub fn build_gemini_files_upstream_url(
    transport: &GatewayProviderTransportSnapshot,
    request_path: &str,
    request_query: Option<&str>,
) -> Option<String> {
    let custom_path = transport
        .endpoint
        .custom_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let passthrough_path = custom_path.unwrap_or(request_path);
    build_gemini_files_passthrough_url(
        &transport.endpoint.base_url,
        passthrough_path,
        request_query,
    )
}

pub fn build_gemini_files_request_body(
    body_json: &Value,
    body_base64: Option<&str>,
    body_is_empty: bool,
    is_upload: bool,
    body_rules: Option<&Value>,
) -> Result<GeminiFilesRequestBodyParts, GeminiFilesRequestBodyError> {
    let mut provider_request_body = if is_upload && !body_is_empty && body_base64.is_none() {
        Some(body_json.clone())
    } else {
        None
    };
    let provider_request_body_base64 = if is_upload {
        body_base64
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    } else {
        None
    };
    if provider_request_body_base64.is_some() && body_rules.is_some() {
        return Err(GeminiFilesRequestBodyError::BodyRulesUnsupportedForBinaryUpload);
    }
    if let Some(body) = provider_request_body.as_mut() {
        if !apply_local_body_rules(body, body_rules, Some(body_json)) {
            return Err(GeminiFilesRequestBodyError::BodyRulesApplyFailed);
        }
    }

    Ok(GeminiFilesRequestBodyParts {
        provider_request_body,
        provider_request_body_base64,
    })
}

pub fn build_gemini_files_headers(
    input: GeminiFilesHeadersInput<'_>,
) -> Option<BTreeMap<String, String>> {
    let mut provider_request_headers = build_passthrough_headers_with_auth(
        input.headers,
        input.auth_header,
        input.auth_value,
        &BTreeMap::new(),
    );
    let null_original_request_body = Value::Null;
    let base64_original_request_body = input
        .provider_request_body_base64
        .map(|body_bytes_b64| json!({ "body_bytes_b64": body_bytes_b64 }));
    let original_request_body = base64_original_request_body
        .as_ref()
        .or_else(|| (!input.original_body_is_empty).then_some(input.original_request_body_json))
        .unwrap_or(&null_original_request_body);
    if !apply_local_header_rules(
        &mut provider_request_headers,
        input.header_rules,
        &[input.auth_header, "content-type"],
        input.provider_request_body.unwrap_or(original_request_body),
        Some(original_request_body),
    ) {
        return None;
    }
    Some(provider_request_headers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };

    fn sample_transport() -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "Provider".to_string(),
                provider_type: "gemini".to_string(),
                website: None,
                is_active: true,
                keep_priority_on_conversion: false,
                enable_format_conversion: false,
                concurrent_limit: None,
                max_retries: None,
                proxy: None,
                request_timeout_secs: None,
                stream_first_byte_timeout_secs: None,
                config: None,
            },
            endpoint: GatewayProviderTransportEndpoint {
                id: "endpoint-1".to_string(),
                provider_id: "provider-1".to_string(),
                api_format: "gemini:files".to_string(),
                api_family: None,
                endpoint_kind: None,
                is_active: true,
                base_url: "https://generativelanguage.googleapis.com".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: None,
                config: None,
                format_acceptance_config: None,
                proxy: None,
            },
            key: GatewayProviderTransportKey {
                id: "key-1".to_string(),
                provider_id: "provider-1".to_string(),
                name: "key".to_string(),
                auth_type: "api_key".to_string(),
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
                decrypted_api_key: "secret".to_string(),
                decrypted_auth_config: None,
            },
        }
    }

    #[test]
    fn builds_gemini_files_url_from_request_path_and_strips_client_key() {
        let url = build_gemini_files_upstream_url(
            &sample_transport(),
            "/v1beta/files/demo",
            Some("key=client-key&alt=json"),
        )
        .expect("url should build");

        assert_eq!(
            url,
            "https://generativelanguage.googleapis.com/v1beta/files/demo?alt=json"
        );
    }

    #[test]
    fn builds_json_upload_body_and_applies_rules() {
        let body = build_gemini_files_request_body(
            &json!({"display_name": "demo"}),
            None,
            false,
            true,
            Some(&json!([
                {"action":"set","path":"metadata.source","value":"local"}
            ])),
        )
        .expect("body should build");

        assert_eq!(
            body.provider_request_body
                .as_ref()
                .and_then(|value| value.pointer("/metadata/source")),
            Some(&json!("local"))
        );
        assert!(body.provider_request_body_base64.is_none());
    }

    #[test]
    fn rejects_body_rules_for_binary_upload() {
        assert_eq!(
            build_gemini_files_request_body(
                &json!({}),
                Some("YWJj"),
                false,
                true,
                Some(&json!([{"action":"set","path":"x","value":1}]))
            ),
            Err(GeminiFilesRequestBodyError::BodyRulesUnsupportedForBinaryUpload)
        );
    }

    #[test]
    fn builds_headers_using_binary_body_as_original_context() {
        let headers = build_gemini_files_headers(GeminiFilesHeadersInput {
            headers: &http::HeaderMap::new(),
            auth_header: "x-goog-api-key",
            auth_value: "secret",
            header_rules: Some(&json!([
                {"action":"set","key":"x-upload-mode","value":"binary"}
            ])),
            provider_request_body: None,
            provider_request_body_base64: Some("YWJj"),
            original_request_body_json: &json!({}),
            original_body_is_empty: false,
        })
        .expect("headers should build");

        assert_eq!(
            headers.get("x-goog-api-key").map(String::as_str),
            Some("secret")
        );
        assert_eq!(
            headers.get("x-upload-mode").map(String::as_str),
            Some("binary")
        );
    }
}
