use serde_json::Value;

use super::super::snapshot::GatewayProviderTransportSnapshot;

pub const GEMINI_CLI_PROVIDER_TYPE: &str = "gemini_cli";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GeminiCliRequestAuth {
    pub project_id: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeminiCliRequestAuthSupport {
    Supported(GeminiCliRequestAuth),
    Unsupported(GeminiCliRequestAuthUnsupportedReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeminiCliRequestAuthUnsupportedReason {
    WrongProviderType,
    InvalidAuthConfigJson,
}

pub fn is_gemini_cli_provider_transport(transport: &GatewayProviderTransportSnapshot) -> bool {
    transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case(GEMINI_CLI_PROVIDER_TYPE)
}

pub fn resolve_local_gemini_cli_request_auth(
    transport: &GatewayProviderTransportSnapshot,
) -> GeminiCliRequestAuthSupport {
    if !is_gemini_cli_provider_transport(transport) {
        return GeminiCliRequestAuthSupport::Unsupported(
            GeminiCliRequestAuthUnsupportedReason::WrongProviderType,
        );
    }

    let metadata = transport.key.upstream_metadata.as_ref();
    let metadata_project_id = metadata.and_then(resolve_project_id_from_value);
    let metadata_session_id = metadata.and_then(resolve_session_id_from_value);

    let Some(raw_auth_config) = transport
        .key
        .decrypted_auth_config
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return GeminiCliRequestAuthSupport::Supported(GeminiCliRequestAuth {
            project_id: metadata_project_id,
            session_id: metadata_session_id,
        });
    };

    let Ok(auth_config) = serde_json::from_str::<Value>(raw_auth_config) else {
        return GeminiCliRequestAuthSupport::Unsupported(
            GeminiCliRequestAuthUnsupportedReason::InvalidAuthConfigJson,
        );
    };

    GeminiCliRequestAuthSupport::Supported(GeminiCliRequestAuth {
        project_id: metadata_project_id.or_else(|| resolve_project_id_from_value(&auth_config)),
        session_id: metadata_session_id.or_else(|| resolve_session_id_from_value(&auth_config)),
    })
}

pub fn resolve_gemini_cli_project_id(
    transport: &GatewayProviderTransportSnapshot,
) -> Option<String> {
    match resolve_local_gemini_cli_request_auth(transport) {
        GeminiCliRequestAuthSupport::Supported(auth) => auth.project_id,
        GeminiCliRequestAuthSupport::Unsupported(_) => None,
    }
}

fn resolve_project_id_from_value(value: &Value) -> Option<String> {
    find_string_by_paths(
        value,
        &[
            &["project"],
            &["project_id"],
            &["projectId"],
            &["project", "id"],
            &["project", "project_id"],
            &["project", "projectId"],
            &["cloudaicompanionProject"],
            &["cloudaicompanionProject", "id"],
            &["cloudaicompanion_project"],
            &["cloudaicompanion_project", "id"],
            &["gemini_cli", "project"],
            &["gemini_cli", "project_id"],
            &["gemini_cli", "projectId"],
            &["gemini_cli", "cloudaicompanionProject"],
            &["gemini_cli", "cloudaicompanionProject", "id"],
            &["geminiCli", "project"],
            &["geminiCli", "project_id"],
            &["geminiCli", "projectId"],
            &["metadata", "project"],
            &["metadata", "project_id"],
            &["metadata", "projectId"],
        ],
    )
}

fn resolve_session_id_from_value(value: &Value) -> Option<String> {
    find_string_by_paths(
        value,
        &[
            &["session_id"],
            &["sessionId"],
            &["gemini_cli", "session_id"],
            &["gemini_cli", "sessionId"],
            &["geminiCli", "session_id"],
            &["geminiCli", "sessionId"],
            &["metadata", "session_id"],
            &["metadata", "sessionId"],
        ],
    )
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
    }

    None
}

#[cfg(test)]
mod tests {
    use super::{
        resolve_gemini_cli_project_id, resolve_local_gemini_cli_request_auth, GeminiCliRequestAuth,
        GeminiCliRequestAuthSupport,
    };
    use crate::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };

    fn sample_transport() -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "Gemini CLI".to_string(),
                provider_type: "gemini_cli".to_string(),
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
                api_family: None,
                endpoint_kind: None,
                is_active: true,
                base_url: "https://cloudcode-pa.googleapis.com".to_string(),
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
                upstream_metadata: None,
                decrypted_api_key: "__oauth__".to_string(),
                decrypted_auth_config: None,
            },
        }
    }

    #[test]
    fn extracts_project_and_session_metadata_when_available() {
        let mut transport = sample_transport();
        transport.key.decrypted_auth_config = Some(
            r#"{
                "cloudaicompanionProject": {"id": "project-123"},
                "metadata": {"sessionId": "session-123"}
            }"#
            .to_string(),
        );

        assert_eq!(
            resolve_local_gemini_cli_request_auth(&transport),
            GeminiCliRequestAuthSupport::Supported(GeminiCliRequestAuth {
                project_id: Some("project-123".to_string()),
                session_id: Some("session-123".to_string()),
            })
        );
    }

    #[test]
    fn project_id_prefers_upstream_metadata_then_auth_config() {
        let mut transport = sample_transport();
        transport.key.upstream_metadata = Some(serde_json::json!({
            "gemini_cli": {
                "project_id": "metadata-project"
            }
        }));
        transport.key.decrypted_auth_config = Some(r#"{"project_id":"auth-project"}"#.to_string());

        assert_eq!(
            resolve_gemini_cli_project_id(&transport).as_deref(),
            Some("metadata-project")
        );

        transport.key.upstream_metadata = None;
        assert_eq!(
            resolve_gemini_cli_project_id(&transport).as_deref(),
            Some("auth-project")
        );
    }

    #[test]
    fn missing_metadata_still_supports_request_envelope() {
        let transport = sample_transport();

        assert_eq!(
            resolve_local_gemini_cli_request_auth(&transport),
            GeminiCliRequestAuthSupport::Supported(GeminiCliRequestAuth {
                project_id: None,
                session_id: None,
            })
        );
    }
}
