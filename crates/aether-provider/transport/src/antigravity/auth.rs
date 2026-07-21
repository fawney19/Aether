use std::collections::BTreeMap;

use serde_json::Value;

use super::super::snapshot::GatewayProviderTransportSnapshot;

pub const ANTIGRAVITY_PROVIDER_TYPE: &str = "antigravity";
pub const ANTIGRAVITY_REQUEST_USER_AGENT: &str =
    "antigravity/cli/1.0.16 (aidev_client; os_type=linux; arch=arm64; auth_method=consumer)";
const ANTIGRAVITY_CLIENT_NAME: &str = "antigravity";
const ANTIGRAVITY_GOOG_API_CLIENT: &str = "gl-node/18.18.2 fire/0.8.6 grpc/1.10.x";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AntigravityRequestAuth {
    pub project_id: String,
    pub client_version: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AntigravityRequestAuthSupport {
    Supported(AntigravityRequestAuth),
    Unsupported(AntigravityRequestAuthUnsupportedReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AntigravityRequestAuthUnsupportedReason {
    WrongProviderType,
    MissingAuthConfig,
    InvalidAuthConfigJson,
    ComplexDynamicAuthConfig,
    MissingProjectId,
}

pub fn resolve_local_antigravity_request_auth(
    transport: &GatewayProviderTransportSnapshot,
) -> AntigravityRequestAuthSupport {
    if !transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case(ANTIGRAVITY_PROVIDER_TYPE)
    {
        return AntigravityRequestAuthSupport::Unsupported(
            AntigravityRequestAuthUnsupportedReason::WrongProviderType,
        );
    }

    let Some(raw_auth_config) = transport
        .key
        .decrypted_auth_config
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return AntigravityRequestAuthSupport::Unsupported(
            AntigravityRequestAuthUnsupportedReason::MissingAuthConfig,
        );
    };

    let Ok(auth_config) = serde_json::from_str::<Value>(raw_auth_config) else {
        return AntigravityRequestAuthSupport::Unsupported(
            AntigravityRequestAuthUnsupportedReason::InvalidAuthConfigJson,
        );
    };

    if contains_blocked_auth_fields(&auth_config) {
        return AntigravityRequestAuthSupport::Unsupported(
            AntigravityRequestAuthUnsupportedReason::ComplexDynamicAuthConfig,
        );
    }

    let upstream_metadata = transport.key.upstream_metadata.as_ref();
    let Some(project_id) = find_antigravity_string(
        upstream_metadata,
        &auth_config,
        ANTIGRAVITY_PROJECT_ID_PATHS,
    ) else {
        return AntigravityRequestAuthSupport::Unsupported(
            AntigravityRequestAuthUnsupportedReason::MissingProjectId,
        );
    };

    let client_version = find_antigravity_string(
        upstream_metadata,
        &auth_config,
        ANTIGRAVITY_CLIENT_VERSION_PATHS,
    );
    let session_id = find_antigravity_string(
        upstream_metadata,
        &auth_config,
        ANTIGRAVITY_SESSION_ID_PATHS,
    );

    AntigravityRequestAuthSupport::Supported(AntigravityRequestAuth {
        project_id,
        client_version,
        session_id,
    })
}

pub fn build_antigravity_static_identity_headers(
    auth: &AntigravityRequestAuth,
) -> BTreeMap<String, String> {
    build_antigravity_static_client_headers(
        auth.client_version.as_deref(),
        auth.session_id.as_deref(),
    )
}

pub fn build_antigravity_static_client_headers(
    client_version: Option<&str>,
    session_id: Option<&str>,
) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::from([
        (
            String::from("x-client-name"),
            String::from(ANTIGRAVITY_CLIENT_NAME),
        ),
        (
            String::from("x-goog-api-client"),
            String::from(ANTIGRAVITY_GOOG_API_CLIENT),
        ),
        (
            String::from("user-agent"),
            String::from(ANTIGRAVITY_REQUEST_USER_AGENT),
        ),
    ]);

    if let Some(client_version) = client_version
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        headers.insert(String::from("x-client-version"), client_version.to_string());
    }
    if let Some(session_id) = session_id.map(str::trim).filter(|value| !value.is_empty()) {
        headers.insert(String::from("x-vscode-sessionid"), session_id.to_string());
    }

    headers
}

const ANTIGRAVITY_PROJECT_ID_PATHS: &[&[&str]] = &[
    &["project_id"],
    &["projectId"],
    &["project", "id"],
    &["project", "project_id"],
    &["project", "projectId"],
    &["cloudaicompanionProject"],
    &["cloudaicompanionProject", "id"],
    &["cloudAiCompanionProject"],
    &["cloudAiCompanionProject", "id"],
    &["antigravity", "project_id"],
    &["antigravity", "projectId"],
    &["antigravity", "project", "id"],
    &["antigravity", "cloudaicompanionProject"],
    &["antigravity", "cloudaicompanionProject", "id"],
    &["antigravity", "cloudAiCompanionProject"],
    &["antigravity", "cloudAiCompanionProject", "id"],
    &["metadata", "project_id"],
    &["metadata", "projectId"],
    &["metadata", "cloudaicompanionProject"],
    &["metadata", "cloudaicompanionProject", "id"],
    &["metadata", "cloudAiCompanionProject"],
    &["metadata", "cloudAiCompanionProject", "id"],
];

const ANTIGRAVITY_CLIENT_VERSION_PATHS: &[&[&str]] = &[
    &["client_version"],
    &["clientVersion"],
    &["antigravity", "client_version"],
    &["antigravity", "clientVersion"],
    &["metadata", "client_version"],
    &["metadata", "clientVersion"],
];

const ANTIGRAVITY_SESSION_ID_PATHS: &[&[&str]] = &[
    &["session_id"],
    &["sessionId"],
    &["antigravity", "session_id"],
    &["antigravity", "sessionId"],
    &["metadata", "session_id"],
    &["metadata", "sessionId"],
];

fn find_antigravity_string(
    upstream_metadata: Option<&Value>,
    auth_config: &Value,
    paths: &[&[&str]],
) -> Option<String> {
    upstream_metadata
        .and_then(|metadata| find_string_by_paths(metadata, paths))
        .or_else(|| find_string_by_paths(auth_config, paths))
}

fn find_string_by_paths(value: &Value, paths: &[&[&str]]) -> Option<String> {
    for path in paths {
        let mut current = value;
        let mut matched = true;
        for segment in *path {
            let Some(next) = current.get(*segment) else {
                matched = false;
                break;
            };
            current = next;
        }
        if !matched {
            continue;
        }
        if let Some(string) = current
            .as_str()
            .map(str::trim)
            .filter(|item| !item.is_empty())
        {
            return Some(string.to_string());
        }
        if let Some(string) = current
            .as_object()
            .and_then(|object| {
                object
                    .get("id")
                    .or_else(|| object.get("project_id"))
                    .or_else(|| object.get("projectId"))
            })
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|item| !item.is_empty())
        {
            return Some(string.to_string());
        }
    }

    None
}

fn contains_blocked_auth_fields(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.iter().any(|(key, inner)| {
            is_blocked_auth_key(key.as_str()) || contains_blocked_auth_fields(inner)
        }),
        Value::Array(items) => items.iter().any(contains_blocked_auth_fields),
        _ => false,
    }
}

fn is_blocked_auth_key(key: &str) -> bool {
    matches!(
        key.trim().to_ascii_lowercase().as_str(),
        "private_key"
            | "privateKey"
            | "private_key_id"
            | "privateKeyId"
            | "service_account"
            | "serviceAccount"
            | "service_account_json"
            | "serviceAccountJson"
            | "service_account_key"
            | "serviceAccountKey"
            | "credential_source"
            | "credentialSource"
            | "token_url"
            | "tokenUrl"
            | "auth_uri"
            | "authUri"
            | "subject"
            | "audience"
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        build_antigravity_static_client_headers, resolve_local_antigravity_request_auth,
        AntigravityRequestAuth, AntigravityRequestAuthSupport, ANTIGRAVITY_REQUEST_USER_AGENT,
    };
    use crate::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };

    fn sample_transport(auth_config: &str) -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "Antigravity".to_string(),
                provider_type: "antigravity".to_string(),
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
                id: "endpoint-1".to_string(),
                provider_id: "provider-1".to_string(),
                api_format: "gemini:generate_content".to_string(),
                api_family: Some("gemini".to_string()),
                endpoint_kind: Some("generate_content".to_string()),
                is_active: true,
                base_url: "https://daily-cloudcode-pa.googleapis.com".to_string(),
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
                auth_type: "oauth".to_string(),
                is_active: true,
                api_formats: Some(vec!["gemini:generate_content".to_string()]),
                auth_type_by_format: None,
                allow_auth_channel_mismatch_formats: None,
                allowed_models: None,
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: None,
                expires_at_unix_secs: None,
                proxy: None,
                fingerprint: None,
                upstream_metadata: None,
                decrypted_api_key: "__placeholder__".to_string(),
                decrypted_auth_config: Some(auth_config.to_string()),
            },
        }
    }

    #[test]
    fn resolves_cloudaicompanion_project_object_from_auth_config() {
        let transport = sample_transport(
            r#"{
                "provider_type":"antigravity",
                "refresh_token":"rt",
                "cloudaicompanionProject":{"id":"project-from-auth-config"}
            }"#,
        );

        assert_eq!(
            resolve_local_antigravity_request_auth(&transport),
            AntigravityRequestAuthSupport::Supported(AntigravityRequestAuth {
                project_id: "project-from-auth-config".to_string(),
                client_version: None,
                session_id: None,
            })
        );
    }

    #[test]
    fn resolves_identity_from_antigravity_upstream_metadata() {
        let mut transport = sample_transport(
            r#"{
                "provider_type":"antigravity",
                "refresh_token":"rt"
            }"#,
        );
        transport.key.upstream_metadata = Some(json!({
            "antigravity": {
                "project_id": "project-from-metadata",
                "client_version": "1.99.0",
                "session_id": "session-from-metadata"
            }
        }));

        assert_eq!(
            resolve_local_antigravity_request_auth(&transport),
            AntigravityRequestAuthSupport::Supported(AntigravityRequestAuth {
                project_id: "project-from-metadata".to_string(),
                client_version: Some("1.99.0".to_string()),
                session_id: Some("session-from-metadata".to_string()),
            })
        );
    }

    #[test]
    fn static_client_headers_use_native_antigravity_cli_user_agent() {
        let headers = build_antigravity_static_client_headers(Some("1.0.16"), Some("session-abc"));

        assert_eq!(
            headers.get("user-agent").map(String::as_str),
            Some(ANTIGRAVITY_REQUEST_USER_AGENT)
        );
        assert_eq!(
            headers.get("x-client-name").map(String::as_str),
            Some("antigravity")
        );
        assert_eq!(
            headers.get("x-client-version").map(String::as_str),
            Some("1.0.16")
        );
        assert_eq!(
            headers.get("x-vscode-sessionid").map(String::as_str),
            Some("session-abc")
        );
    }
}
