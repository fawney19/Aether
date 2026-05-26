use std::collections::BTreeMap;

use serde_json::Value;

use super::super::snapshot::GatewayProviderTransportSnapshot;

pub const ANTIGRAVITY_PROVIDER_TYPE: &str = "antigravity";
pub const ANTIGRAVITY_CLI_PROVIDER_TYPE: &str = "antigravity_cli";
pub const ANTIGRAVITY_REQUEST_USER_AGENT: &str = "antigravity";
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

pub fn is_antigravity_runtime_provider_type(provider_type: &str) -> bool {
    let normalized = provider_type.trim().to_ascii_lowercase();
    normalized == ANTIGRAVITY_PROVIDER_TYPE || normalized == ANTIGRAVITY_CLI_PROVIDER_TYPE
}

pub fn resolve_local_antigravity_request_auth(
    transport: &GatewayProviderTransportSnapshot,
) -> AntigravityRequestAuthSupport {
    if !is_antigravity_runtime_provider_type(&transport.provider.provider_type) {
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

    let Some(project_id) = find_string_by_paths(
        &auth_config,
        &[
            &["project_id"],
            &["projectId"],
            &["project", "id"],
            &["project", "project_id"],
            &["project", "projectId"],
            &["antigravity", "project_id"],
            &["antigravity", "projectId"],
            &["metadata", "project_id"],
            &["metadata", "projectId"],
        ],
    ) else {
        return AntigravityRequestAuthSupport::Unsupported(
            AntigravityRequestAuthUnsupportedReason::MissingProjectId,
        );
    };

    let client_version = find_string_by_paths(
        &auth_config,
        &[
            &["client_version"],
            &["clientVersion"],
            &["antigravity", "client_version"],
            &["antigravity", "clientVersion"],
            &["metadata", "client_version"],
            &["metadata", "clientVersion"],
        ],
    );
    let session_id = find_string_by_paths(
        &auth_config,
        &[
            &["session_id"],
            &["sessionId"],
            &["antigravity", "session_id"],
            &["antigravity", "sessionId"],
            &["metadata", "session_id"],
            &["metadata", "sessionId"],
        ],
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
    let mut headers = BTreeMap::from([
        (
            String::from("x-client-name"),
            String::from(ANTIGRAVITY_CLIENT_NAME),
        ),
        (
            String::from("x-goog-api-client"),
            String::from(ANTIGRAVITY_GOOG_API_CLIENT),
        ),
    ]);

    if let Some(client_version) = auth
        .client_version
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        headers.insert(String::from("x-client-version"), client_version.to_string());
    }
    if let Some(session_id) = auth
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        headers.insert(String::from("x-vscode-sessionid"), session_id.to_string());
    }

    headers
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
