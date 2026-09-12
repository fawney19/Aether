use std::collections::BTreeMap;

use aether_oauth::network::{OAuthHttpExecutor, OAuthHttpRequest, OAuthNetworkContext};
use async_trait::async_trait;
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use chrono::{DateTime, SecondsFormat, Utc};
use crypto_box::{
    aead::rand_core::{OsRng, RngCore},
    SecretKey as Curve25519SecretKey,
};
use ed25519_dalek::{
    pkcs8::{DecodePrivateKey, EncodePrivateKey},
    Signature, Signer, SigningKey, Verifier,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256, Sha512};
use thiserror::Error;
use url::Url;

use super::oauth_refresh::{
    CachedOAuthEntry, LocalOAuthHttpExecutor, LocalOAuthHttpRequest, LocalOAuthRefreshAdapter,
    LocalOAuthRefreshError, LocalResolvedOAuthRequestAuth,
};
use super::snapshot::GatewayProviderTransportSnapshot;

pub const CODEX_AGENT_IDENTITY_AUTH_MODE: &str = "agentIdentity";
pub const CODEX_AGENT_IDENTITY_PROVIDER_TYPE: &str = "codex";
pub const CODEX_AGENT_IDENTITY_CACHED_ENTRY_PROVIDER_TYPE: &str = "codex_agent_identity";
pub const CODEX_AGENT_IDENTITY_AGENT_REGISTRATION_REQUEST_ID: &str =
    "codex:agent-identity-agent-register";
pub const CODEX_AGENT_IDENTITY_TASK_REGISTRATION_REQUEST_ID: &str =
    "codex:agent-identity-task-register";
const CODEX_AGENT_IDENTITY_AUTH_API_BASE_URL: &str = "https://auth.openai.com/api/accounts";
const AUTHORIZATION_HEADER: &str = "authorization";
const ASSERTION_PREFIX: &str = "AgentAssertion ";
const CODEX_AGENT_IDENTITY_AGENT_HARNESS_ID: &str = "codex-cli";
const CODEX_AGENT_IDENTITY_RESPONSES_API_CAPABILITY: &str = "responsesapi";

/// The AgentAssertion scheme is generated internally after an Agent Identity
/// task has been registered.  Keep the scheme check separate from envelope
/// validation so a malformed in-flight assertion is still treated as an
/// Agent-originated request by defensive runtime state writers.
pub fn is_codex_agent_identity_authorization(value: &str) -> bool {
    encoded_agent_identity_assertion(value).is_some()
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CodexAgentIdentityEnrollmentError {
    #[error("ChatGPT Access Token 不能为空")]
    MissingSessionToken,
    #[error("Agent Identity 注册请求失败")]
    RegistrationRequestFailed,
    #[error("Agent Identity 注册被拒绝（HTTP {status_code}）{reason}")]
    RegistrationRejected {
        status_code: u16,
        reason: &'static str,
    },
    #[error("Agent Identity 注册响应无效")]
    InvalidRegistrationResponse,
    #[error("Agent Identity 密钥生成失败")]
    KeyGenerationFailed,
    #[error("Agent Identity task 初始化请求失败")]
    TaskRegistrationRequestFailed,
    #[error("Agent Identity task 初始化被拒绝（HTTP {status_code}）")]
    TaskRegistrationRejected { status_code: u16 },
    #[error("Agent Identity task 初始化响应无效")]
    InvalidTaskRegistrationResponse,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CodexAgentIdentityRegistrationOptions {
    pub is_fedramp_account: bool,
}

#[derive(Clone)]
struct AgentIdentityCredentials {
    runtime_id: String,
    signing_key: SigningKey,
    task_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AgentIdentityAssertionEnvelope {
    agent_runtime_id: String,
    task_id: String,
    timestamp: String,
    signature: String,
}

const MAX_AGENT_IDENTITY_PRIVATE_KEY_DER_BYTES: usize = 4 * 1024;
const MAX_AGENT_IDENTITY_ASSERTION_ENVELOPE_BYTES: usize = 64 * 1024;
const MAX_AGENT_IDENTITY_SIGNATURE_BYTES: usize = 128;
const MAX_AGENT_IDENTITY_ENCRYPTED_TASK_ID_BYTES: usize = 64 * 1024;

fn maximum_base64_len_for_decoded_limit(limit: usize) -> usize {
    limit
        .saturating_add(2)
        .checked_div(3)
        .unwrap_or(usize::MAX)
        .saturating_mul(4)
}

fn decode_standard_base64_with_limit(value: &str, limit: usize) -> Option<Vec<u8>> {
    if value.len() > maximum_base64_len_for_decoded_limit(limit) {
        return None;
    }
    let decoded = STANDARD.decode(value).ok()?;
    (decoded.len() <= limit).then_some(decoded)
}

fn decode_urlsafe_base64_with_limit(value: &str, limit: usize) -> Option<Vec<u8>> {
    if value.len() > maximum_base64_len_for_decoded_limit(limit) {
        return None;
    }
    let decoded = URL_SAFE_NO_PAD.decode(value).ok()?;
    (decoded.len() <= limit).then_some(decoded)
}

#[derive(Debug, Deserialize)]
struct AgentTaskRegistrationResponse {
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default, rename = "taskId")]
    task_id_camel: Option<String>,
    #[serde(default)]
    encrypted_task_id: Option<String>,
    #[serde(default, rename = "encryptedTaskId")]
    encrypted_task_id_camel: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AgentRegistrationResponse {
    #[serde(default)]
    agent_runtime_id: Option<String>,
    #[serde(default, rename = "agentRuntimeId")]
    agent_runtime_id_camel: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CodexAgentIdentityRefreshAdapter {
    auth_api_base_url: String,
}

impl Default for CodexAgentIdentityRefreshAdapter {
    fn default() -> Self {
        Self {
            auth_api_base_url: CODEX_AGENT_IDENTITY_AUTH_API_BASE_URL.to_string(),
        }
    }
}

impl CodexAgentIdentityRefreshAdapter {
    pub fn with_auth_api_base_url_for_tests(mut self, base_url: impl Into<String>) -> Self {
        self.auth_api_base_url = base_url.into();
        self
    }

    fn config_from_transport(transport: &GatewayProviderTransportSnapshot) -> Option<Value> {
        transport
            .key
            .decrypted_auth_config
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(|value| serde_json::from_str::<Value>(value).ok())
    }

    fn config_from_entry(entry: &CachedOAuthEntry) -> Option<Value> {
        entry
            .provider_type
            .trim()
            .eq_ignore_ascii_case(CODEX_AGENT_IDENTITY_CACHED_ENTRY_PROVIDER_TYPE)
            .then(|| entry.metadata.clone())
            .flatten()
    }

    fn resolve_from_config(config: &Value) -> Option<LocalResolvedOAuthRequestAuth> {
        let credentials = agent_identity_credentials(config).ok()?;
        let task_id = credentials.task_id.as_deref()?;
        let value = build_agent_assertion(&credentials, task_id, Utc::now()).ok()?;
        Some(LocalResolvedOAuthRequestAuth::Header {
            name: AUTHORIZATION_HEADER.to_string(),
            value,
        })
    }

    fn preferred_config(
        transport: &GatewayProviderTransportSnapshot,
        entry: Option<&CachedOAuthEntry>,
    ) -> Option<Value> {
        Self::config_from_transport(transport).or_else(|| entry.and_then(Self::config_from_entry))
    }
}

/// Returns a stable, non-secret fingerprint for the Agent Identity key pair
/// and runtime. The task id is deliberately excluded so task rotation does not
/// look like a credential replacement.
pub fn codex_agent_identity_credential_fingerprint(config: &Value) -> Option<String> {
    let credentials = agent_identity_credentials(config).ok()?;
    Some(agent_identity_credential_fingerprint_from_credentials(
        &credentials,
    ))
}

pub fn codex_agent_identity_transport_credential_fingerprint(
    transport: &GatewayProviderTransportSnapshot,
) -> Option<String> {
    CodexAgentIdentityRefreshAdapter::config_from_transport(transport)
        .and_then(|config| codex_agent_identity_credential_fingerprint(&config))
}

/// Returns the fencing generation for a transport/config entry, including the
/// current task id. It is safe to log/compare but must never be used as a key.
pub fn codex_agent_identity_refresh_fingerprint(
    transport: &GatewayProviderTransportSnapshot,
    entry: Option<&CachedOAuthEntry>,
) -> Option<String> {
    let transport_config = CodexAgentIdentityRefreshAdapter::config_from_transport(transport)?;
    let transport_credentials = agent_identity_credentials(&transport_config).ok()?;
    let transport_credential_fingerprint =
        agent_identity_credential_fingerprint_from_credentials(&transport_credentials);
    let config = entry
        .filter(|entry| {
            entry.source_fingerprint.as_deref() == Some(transport_credential_fingerprint.as_str())
        })
        .and_then(CodexAgentIdentityRefreshAdapter::config_from_entry)
        .filter(|entry_config| {
            agent_identity_credentials(entry_config)
                .ok()
                .is_some_and(|entry_credentials| {
                    entry_credentials.task_id == transport_credentials.task_id
                })
        })
        .unwrap_or(transport_config);
    codex_agent_identity_config_refresh_fingerprint(&config)
}

pub fn codex_agent_identity_config_refresh_fingerprint(config: &Value) -> Option<String> {
    let credentials = agent_identity_credentials(config).ok()?;
    let credential_fingerprint =
        agent_identity_credential_fingerprint_from_credentials(&credentials);
    let task_id = credentials.task_id.as_deref().unwrap_or_default();
    let mut digest = Sha256::new();
    digest.update(credential_fingerprint.as_bytes());
    digest.update([0]);
    digest.update(task_id.as_bytes());
    Some(URL_SAFE_NO_PAD.encode(digest.finalize()))
}

pub fn codex_agent_identity_cached_entry_from_transport(
    transport: &GatewayProviderTransportSnapshot,
) -> Option<CachedOAuthEntry> {
    let config = CodexAgentIdentityRefreshAdapter::config_from_transport(transport)?;
    let credentials = agent_identity_credentials(&config).ok()?;
    let task_id = credentials.task_id.as_deref()?;
    let auth_header_value = build_agent_assertion(&credentials, task_id, Utc::now()).ok()?;
    Some(CachedOAuthEntry {
        provider_type: CODEX_AGENT_IDENTITY_CACHED_ENTRY_PROVIDER_TYPE.to_string(),
        auth_header_name: AUTHORIZATION_HEADER.to_string(),
        auth_header_value,
        expires_at_unix_secs: None,
        metadata: Some(config),
        source_fingerprint: Some(agent_identity_credential_fingerprint_from_credentials(
            &credentials,
        )),
    })
}

fn agent_identity_credential_fingerprint_from_credentials(
    credentials: &AgentIdentityCredentials,
) -> String {
    let mut digest = Sha256::new();
    digest.update(credentials.runtime_id.as_bytes());
    digest.update([0]);
    digest.update(credentials.signing_key.to_bytes());
    URL_SAFE_NO_PAD.encode(digest.finalize())
}

pub fn is_codex_agent_identity_auth_config_value(config: &Value) -> bool {
    let Some(root) = config.as_object() else {
        return false;
    };
    let nested = agent_identity_nested_object(root);
    let mode = string_from_maps(root, nested, &["auth_mode", "authMode"]);
    mode.as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case(CODEX_AGENT_IDENTITY_AUTH_MODE))
        || (nested.is_some()
            && string_from_maps(root, nested, &["agent_runtime_id", "agentRuntimeId"]).is_some()
            && string_from_maps(root, nested, &["agent_private_key", "agentPrivateKey"]).is_some())
}

pub fn is_codex_agent_identity_transport(transport: &GatewayProviderTransportSnapshot) -> bool {
    transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case(CODEX_AGENT_IDENTITY_PROVIDER_TYPE)
        && transport.key.auth_type.trim().eq_ignore_ascii_case("oauth")
        && CodexAgentIdentityRefreshAdapter::config_from_transport(transport)
            .as_ref()
            .is_some_and(is_codex_agent_identity_auth_config_value)
}

/// Verifies that an in-flight AgentAssertion was signed by the exact Agent
/// Identity credential and task represented by the current transport.
pub fn codex_agent_identity_authorization_matches_transport(
    transport: &GatewayProviderTransportSnapshot,
    authorization: &str,
) -> bool {
    if !is_codex_agent_identity_transport(transport) {
        return false;
    }
    let Some(encoded) = encoded_agent_identity_assertion(authorization) else {
        return false;
    };
    let Some(envelope_bytes) =
        decode_urlsafe_base64_with_limit(encoded, MAX_AGENT_IDENTITY_ASSERTION_ENVELOPE_BYTES)
    else {
        return false;
    };
    let Ok(envelope) = serde_json::from_slice::<AgentIdentityAssertionEnvelope>(&envelope_bytes)
    else {
        return false;
    };
    let Some(config) = CodexAgentIdentityRefreshAdapter::config_from_transport(transport) else {
        return false;
    };
    let Ok(credentials) = agent_identity_credentials(&config) else {
        return false;
    };
    let Some(current_task_id) = credentials.task_id.as_deref() else {
        return false;
    };
    let runtime_id = envelope.agent_runtime_id.trim();
    let task_id = envelope.task_id.trim();
    let timestamp = envelope.timestamp.trim();
    if runtime_id != credentials.runtime_id || task_id != current_task_id || timestamp.is_empty() {
        return false;
    }
    let Some(signature_bytes) = decode_standard_base64_with_limit(
        envelope.signature.trim(),
        MAX_AGENT_IDENTITY_SIGNATURE_BYTES,
    ) else {
        return false;
    };
    let Ok(signature) = Signature::from_slice(&signature_bytes) else {
        return false;
    };
    let payload = format!("{runtime_id}:{task_id}:{timestamp}");
    credentials
        .signing_key
        .verifying_key()
        .verify(payload.as_bytes(), &signature)
        .is_ok()
}

/// Agent task rotation may change only task_id. Any other auth_config change
/// means the caller must rebuild the whole request from a fresh transport.
pub fn codex_agent_identity_transport_allows_task_rotation_from(
    initial: &GatewayProviderTransportSnapshot,
    current: &GatewayProviderTransportSnapshot,
) -> bool {
    let Some(initial_config) = CodexAgentIdentityRefreshAdapter::config_from_transport(initial)
        .and_then(agent_identity_config_without_task)
    else {
        return false;
    };
    let Some(current_config) = CodexAgentIdentityRefreshAdapter::config_from_transport(current)
        .and_then(agent_identity_config_without_task)
    else {
        return false;
    };
    initial_config == current_config
}

pub fn codex_agent_identity_entry_allows_task_rotation_from(
    initial: &GatewayProviderTransportSnapshot,
    entry: &CachedOAuthEntry,
) -> bool {
    let Some(initial_config) = CodexAgentIdentityRefreshAdapter::config_from_transport(initial)
        .and_then(agent_identity_config_without_task)
    else {
        return false;
    };
    let Some(entry_config) = CodexAgentIdentityRefreshAdapter::config_from_entry(entry)
        .and_then(agent_identity_config_without_task)
    else {
        return false;
    };
    initial_config == entry_config
}

pub fn is_codex_agent_identity_cached_entry(entry: &CachedOAuthEntry) -> bool {
    entry
        .provider_type
        .trim()
        .eq_ignore_ascii_case(CODEX_AGENT_IDENTITY_CACHED_ENTRY_PROVIDER_TYPE)
}

pub fn validate_codex_agent_identity_auth_config(config: &Value) -> Result<(), String> {
    agent_identity_credentials(config).map(|_| ())
}

pub fn codex_agent_identity_auth_config_has_task_id(config: &Value) -> bool {
    let Some(root) = config.as_object() else {
        return false;
    };
    string_from_maps(
        root,
        agent_identity_nested_object(root),
        &["task_id", "taskId"],
    )
    .is_some()
}

/// Returns whether an upstream response proves that the registered Agent Identity task is no
/// longer usable. Only this condition should trigger task registration again; an arbitrary 401
/// can instead mean that the account itself has lost access.
pub fn is_codex_agent_identity_invalid_task_response(
    status_code: u16,
    response_text: Option<&str>,
) -> bool {
    if status_code != 401 {
        return false;
    }
    let Some(response_text) = response_text else {
        return false;
    };
    let lower = response_text.to_ascii_lowercase();
    let compact = lower
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect::<String>();
    if [
        r#""code":"invalid_task_id""#,
        r#""code":"task_not_found""#,
        r#""code":"task_expired""#,
        r#""error":"invalid_task_id""#,
    ]
    .iter()
    .any(|marker| compact.contains(marker))
    {
        return true;
    }
    [
        "invalid task_id",
        "invalid task id",
        "task_id is invalid",
        "task id is invalid",
        "task not found",
        "task expired",
        "unknown task_id",
        "unknown task id",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn agent_identity_nested_object(root: &Map<String, Value>) -> Option<&Map<String, Value>> {
    root.get("agent_identity")
        .or_else(|| root.get("agentIdentity"))
        .and_then(Value::as_object)
}

fn agent_identity_config_without_task(mut config: Value) -> Option<Value> {
    if !is_codex_agent_identity_auth_config_value(&config) {
        return None;
    }
    let root = config.as_object_mut()?;
    root.remove("task_id");
    root.remove("taskId");
    for nested_key in ["agent_identity", "agentIdentity"] {
        if let Some(nested) = root.get_mut(nested_key).and_then(Value::as_object_mut) {
            nested.remove("task_id");
            nested.remove("taskId");
        }
    }
    Some(config)
}

fn string_from_map(map: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        map.get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn string_from_maps(
    root: &Map<String, Value>,
    nested: Option<&Map<String, Value>>,
    keys: &[&str],
) -> Option<String> {
    nested
        .and_then(|nested| string_from_map(nested, keys))
        .or_else(|| string_from_map(root, keys))
}

fn agent_identity_credentials(config: &Value) -> Result<AgentIdentityCredentials, String> {
    let root = config
        .as_object()
        .ok_or_else(|| "Agent Identity auth_config must be a JSON object".to_string())?;
    if !is_codex_agent_identity_auth_config_value(config) {
        return Err("Codex Agent Identity auth_mode must be agentIdentity".to_string());
    }
    let nested = agent_identity_nested_object(root);
    let runtime_id = string_from_maps(root, nested, &["agent_runtime_id", "agentRuntimeId"])
        .ok_or_else(|| "Agent Identity agent_runtime_id is required".to_string())?;
    let encoded_private_key =
        string_from_maps(root, nested, &["agent_private_key", "agentPrivateKey"])
            .ok_or_else(|| "Agent Identity agent_private_key is required".to_string())?;
    let private_key_der = decode_standard_base64_with_limit(
        &encoded_private_key,
        MAX_AGENT_IDENTITY_PRIVATE_KEY_DER_BYTES,
    )
    .ok_or_else(|| "Agent Identity agent_private_key must be base64 PKCS#8".to_string())?;
    let signing_key = SigningKey::from_pkcs8_der(&private_key_der).map_err(|_| {
        "Agent Identity agent_private_key must be an Ed25519 PKCS#8 key".to_string()
    })?;

    Ok(AgentIdentityCredentials {
        runtime_id,
        signing_key,
        task_id: string_from_maps(root, nested, &["task_id", "taskId"]),
    })
}

fn agent_identity_timestamp(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn encoded_agent_identity_assertion(value: &str) -> Option<&str> {
    let mut parts = value.split_ascii_whitespace();
    let scheme = parts.next()?;
    let encoded = parts.next()?;
    if !scheme.eq_ignore_ascii_case(ASSERTION_PREFIX.trim())
        || encoded.is_empty()
        || parts.next().is_some()
    {
        return None;
    }
    Some(encoded)
}

fn build_agent_assertion(
    credentials: &AgentIdentityCredentials,
    task_id: &str,
    now: DateTime<Utc>,
) -> Result<String, String> {
    let task_id = task_id.trim();
    if task_id.is_empty() {
        return Err("Agent Identity task_id is required".to_string());
    }
    let timestamp = agent_identity_timestamp(now);
    let payload = format!("{}:{task_id}:{timestamp}", credentials.runtime_id);
    let signature = credentials.signing_key.sign(payload.as_bytes());
    let envelope = json!({
        "agent_runtime_id": credentials.runtime_id,
        "task_id": task_id,
        "timestamp": timestamp,
        "signature": STANDARD.encode(signature.to_bytes()),
    });
    let encoded = serde_json::to_vec(&envelope)
        .map_err(|_| "failed to serialize Agent Identity assertion".to_string())?;
    Ok(format!(
        "{ASSERTION_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(encoded)
    ))
}

fn build_task_registration_signature(
    credentials: &AgentIdentityCredentials,
    now: DateTime<Utc>,
) -> (String, String) {
    let timestamp = agent_identity_timestamp(now);
    let payload = format!("{}:{timestamp}", credentials.runtime_id);
    let signature = credentials.signing_key.sign(payload.as_bytes());
    (timestamp, STANDARD.encode(signature.to_bytes()))
}

fn task_registration_url(base_url: &str, runtime_id: &str) -> Result<String, String> {
    let mut url = Url::parse(base_url.trim())
        .map_err(|_| "Agent Identity auth API base URL is invalid".to_string())?;
    let mut segments = url
        .path_segments_mut()
        .map_err(|_| "Agent Identity auth API base URL cannot be a base URL".to_string())?;
    segments.pop_if_empty();
    for segment in ["v1", "agent", runtime_id, "task", "register"] {
        segments.push(segment);
    }
    drop(segments);
    Ok(url.into())
}

fn agent_registration_url(base_url: &str) -> Result<String, String> {
    let mut url = Url::parse(base_url.trim())
        .map_err(|_| "Agent Identity auth API base URL is invalid".to_string())?;
    let mut segments = url
        .path_segments_mut()
        .map_err(|_| "Agent Identity auth API base URL cannot be a base URL".to_string())?;
    segments.pop_if_empty();
    for segment in ["v1", "agent", "register"] {
        segments.push(segment);
    }
    drop(segments);
    Ok(url.into())
}

fn generate_agent_identity_signing_key() -> SigningKey {
    let mut seed = [0_u8; 32];
    let mut rng = OsRng;
    rng.fill_bytes(&mut seed);
    let signing_key = SigningKey::from_bytes(&seed);
    seed.fill(0);
    signing_key
}

fn agent_identity_ssh_public_key(signing_key: &SigningKey) -> String {
    let header = b"ssh-ed25519";
    let public_key = signing_key.verifying_key().to_bytes();
    let mut blob = Vec::with_capacity(4 + header.len() + 4 + public_key.len());
    blob.extend_from_slice(&(header.len() as u32).to_be_bytes());
    blob.extend_from_slice(header);
    blob.extend_from_slice(&(public_key.len() as u32).to_be_bytes());
    blob.extend_from_slice(&public_key);
    format!("ssh-ed25519 {}", STANDARD.encode(blob))
}

fn agent_runtime_id_from_registration_response(body: &str) -> Result<String, ()> {
    let response = serde_json::from_str::<AgentRegistrationResponse>(body).map_err(|_| ())?;
    [response.agent_runtime_id, response.agent_runtime_id_camel]
        .into_iter()
        .flatten()
        .map(|value| value.trim().to_string())
        .find(|value| !value.is_empty())
        .ok_or(())
}

fn agent_identity_registration_rejection_reason(status_code: u16) -> &'static str {
    match status_code {
        401 => "：AccessToken 无效或已过期",
        403 => {
            "：账户、工作区或出口区域未获服务端 Agent Identity 资格；该资格无法由 Aether 本地开通"
        }
        429 => "：请求过于频繁，请稍后重试",
        _ => "",
    }
}

/// Uses a ChatGPT access token once to register a fresh Agent Identity. The returned config
/// contains only the generated signing credentials and is deliberately free of the access token.
pub async fn register_codex_agent_identity_from_access_token(
    executor: &dyn OAuthHttpExecutor,
    access_token: &str,
    options: CodexAgentIdentityRegistrationOptions,
    network: OAuthNetworkContext,
) -> Result<Map<String, Value>, CodexAgentIdentityEnrollmentError> {
    register_codex_agent_identity_from_access_token_with_auth_api_base_url(
        executor,
        access_token,
        options,
        network,
        CODEX_AGENT_IDENTITY_AUTH_API_BASE_URL,
    )
    .await
}

/// Registers an Agent Identity and its initial task for backwards compatibility.
pub async fn create_codex_agent_identity_from_access_token(
    executor: &dyn OAuthHttpExecutor,
    access_token: &str,
    network: OAuthNetworkContext,
) -> Result<Map<String, Value>, CodexAgentIdentityEnrollmentError> {
    create_codex_agent_identity_from_session_token_with_auth_api_base_url(
        executor,
        access_token,
        network,
        CODEX_AGENT_IDENTITY_AUTH_API_BASE_URL,
    )
    .await
}

/// Backwards-compatible alias for callers using the original, inaccurate name.
pub async fn create_codex_agent_identity_from_session_token(
    executor: &dyn OAuthHttpExecutor,
    session_token: &str,
    network: OAuthNetworkContext,
) -> Result<Map<String, Value>, CodexAgentIdentityEnrollmentError> {
    create_codex_agent_identity_from_access_token(executor, session_token, network).await
}

async fn create_codex_agent_identity_from_session_token_with_auth_api_base_url(
    executor: &dyn OAuthHttpExecutor,
    session_token: &str,
    network: OAuthNetworkContext,
    auth_api_base_url: &str,
) -> Result<Map<String, Value>, CodexAgentIdentityEnrollmentError> {
    let mut auth_config = register_codex_agent_identity_from_access_token_with_auth_api_base_url(
        executor,
        session_token,
        CodexAgentIdentityRegistrationOptions::default(),
        network.clone(),
        auth_api_base_url,
    )
    .await?;
    let config_value = Value::Object(auth_config.clone());
    let credentials = agent_identity_credentials(&config_value)
        .map_err(|_| CodexAgentIdentityEnrollmentError::KeyGenerationFailed)?;
    let (timestamp, signature) = build_task_registration_signature(&credentials, Utc::now());
    let task_url = task_registration_url(auth_api_base_url, credentials.runtime_id.as_str())
        .map_err(|_| CodexAgentIdentityEnrollmentError::TaskRegistrationRequestFailed)?;
    let task_response = executor
        .execute(OAuthHttpRequest {
            request_id: CODEX_AGENT_IDENTITY_TASK_REGISTRATION_REQUEST_ID.to_string(),
            method: reqwest::Method::POST,
            url: task_url,
            headers: BTreeMap::from([
                ("accept".to_string(), "application/json".to_string()),
                ("content-type".to_string(), "application/json".to_string()),
            ]),
            content_type: Some("application/json".to_string()),
            json_body: Some(json!({
                "timestamp": timestamp,
                "signature": signature,
            })),
            body_bytes: None,
            network,
            transport_profile: None,
        })
        .await
        .map_err(|_| CodexAgentIdentityEnrollmentError::TaskRegistrationRequestFailed)?;
    if !(200..300).contains(&task_response.status_code) {
        return Err(
            CodexAgentIdentityEnrollmentError::TaskRegistrationRejected {
                status_code: task_response.status_code,
            },
        );
    }
    let task_id =
        task_id_from_registration_response(&credentials, task_response.body_text.as_str())
            .map_err(|_| CodexAgentIdentityEnrollmentError::InvalidTaskRegistrationResponse)?;
    auth_config.insert("task_id".to_string(), Value::String(task_id));
    validate_codex_agent_identity_auth_config(&Value::Object(auth_config.clone()))
        .map_err(|_| CodexAgentIdentityEnrollmentError::KeyGenerationFailed)?;
    Ok(auth_config)
}

async fn register_codex_agent_identity_from_access_token_with_auth_api_base_url(
    executor: &dyn OAuthHttpExecutor,
    access_token: &str,
    options: CodexAgentIdentityRegistrationOptions,
    network: OAuthNetworkContext,
    auth_api_base_url: &str,
) -> Result<Map<String, Value>, CodexAgentIdentityEnrollmentError> {
    let access_token = access_token.trim();
    if access_token.is_empty() {
        return Err(CodexAgentIdentityEnrollmentError::MissingSessionToken);
    }

    let signing_key = generate_agent_identity_signing_key();
    let private_key_der = signing_key
        .to_pkcs8_der()
        .map_err(|_| CodexAgentIdentityEnrollmentError::KeyGenerationFailed)?;
    let agent_private_key = STANDARD.encode(private_key_der.as_bytes());
    let agent_public_key = agent_identity_ssh_public_key(&signing_key);
    let registration_url = agent_registration_url(auth_api_base_url)
        .map_err(|_| CodexAgentIdentityEnrollmentError::RegistrationRequestFailed)?;
    let mut headers = BTreeMap::from([
        ("accept".to_string(), "application/json".to_string()),
        ("content-type".to_string(), "application/json".to_string()),
        (
            "authorization".to_string(),
            format!("Bearer {access_token}"),
        ),
        (
            "user-agent".to_string(),
            aether_ai_formats::CODEX_CLIENT_USER_AGENT.to_string(),
        ),
        (
            "originator".to_string(),
            aether_ai_formats::CODEX_CLIENT_ORIGINATOR.to_string(),
        ),
    ]);
    if options.is_fedramp_account {
        headers.insert("x-openai-fedramp".to_string(), "true".to_string());
    }
    let response = executor
        .execute(OAuthHttpRequest {
            request_id: CODEX_AGENT_IDENTITY_AGENT_REGISTRATION_REQUEST_ID.to_string(),
            method: reqwest::Method::POST,
            url: registration_url,
            headers,
            content_type: Some("application/json".to_string()),
            json_body: Some(json!({
                "abom": {
                    "agent_version": aether_ai_formats::CODEX_CLIENT_VERSION,
                    "agent_harness_id": CODEX_AGENT_IDENTITY_AGENT_HARNESS_ID,
                    "running_location": format!("cli-{}", std::env::consts::OS),
                },
                "agent_public_key": agent_public_key,
                "capabilities": [CODEX_AGENT_IDENTITY_RESPONSES_API_CAPABILITY],
                "ttl": null,
            })),
            body_bytes: None,
            network,
            transport_profile: None,
        })
        .await
        .map_err(|_| CodexAgentIdentityEnrollmentError::RegistrationRequestFailed)?;
    if !(200..300).contains(&response.status_code) {
        return Err(CodexAgentIdentityEnrollmentError::RegistrationRejected {
            status_code: response.status_code,
            reason: agent_identity_registration_rejection_reason(response.status_code),
        });
    }
    let agent_runtime_id = agent_runtime_id_from_registration_response(response.body_text.as_str())
        .map_err(|_| CodexAgentIdentityEnrollmentError::InvalidRegistrationResponse)?;
    let auth_config = Map::from_iter([
        (
            "provider_type".to_string(),
            json!(CODEX_AGENT_IDENTITY_PROVIDER_TYPE),
        ),
        (
            "auth_mode".to_string(),
            json!(CODEX_AGENT_IDENTITY_AUTH_MODE),
        ),
        ("agent_runtime_id".to_string(), json!(agent_runtime_id)),
        ("agent_private_key".to_string(), json!(agent_private_key)),
    ]);
    validate_codex_agent_identity_auth_config(&Value::Object(auth_config.clone()))
        .map_err(|_| CodexAgentIdentityEnrollmentError::KeyGenerationFailed)?;
    Ok(auth_config)
}

fn task_id_from_registration_response(
    credentials: &AgentIdentityCredentials,
    body: &str,
) -> Result<String, String> {
    let response = serde_json::from_str::<AgentTaskRegistrationResponse>(body)
        .map_err(|_| "Agent Identity task registration returned invalid JSON".to_string())?;
    for task_id in [response.task_id, response.task_id_camel]
        .into_iter()
        .flatten()
    {
        let task_id = task_id.trim();
        if !task_id.is_empty() {
            return Ok(task_id.to_string());
        }
    }
    let encrypted_task_id = [response.encrypted_task_id, response.encrypted_task_id_camel]
        .into_iter()
        .flatten()
        .find(|value| !value.trim().is_empty())
        .ok_or_else(|| "Agent Identity task registration omitted task_id".to_string())?;
    decrypt_agent_task_id(credentials, encrypted_task_id.as_str())
}

fn decrypt_agent_task_id(
    credentials: &AgentIdentityCredentials,
    encrypted_task_id: &str,
) -> Result<String, String> {
    let ciphertext = decode_standard_base64_with_limit(
        encrypted_task_id.trim(),
        MAX_AGENT_IDENTITY_ENCRYPTED_TASK_ID_BYTES,
    )
    .ok_or_else(|| "Agent Identity encrypted_task_id must be valid bounded base64".to_string())?;
    let seed = credentials.signing_key.to_bytes();
    let digest = Sha512::digest(seed);
    let mut curve_private_key = [0u8; 32];
    curve_private_key.copy_from_slice(&digest[..32]);
    let secret_key = Curve25519SecretKey::from_bytes(curve_private_key);
    let plaintext = secret_key
        .unseal(&ciphertext)
        .map_err(|_| "Agent Identity encrypted_task_id could not be decrypted".to_string())?;
    let task_id = String::from_utf8(plaintext)
        .map_err(|_| "Agent Identity decrypted task_id is invalid".to_string())?;
    let task_id = task_id.trim();
    if task_id.is_empty() {
        return Err("Agent Identity decrypted task_id is empty".to_string());
    }
    Ok(task_id.to_string())
}

fn with_agent_identity_task_id(config: &Value, task_id: String) -> Result<Value, String> {
    let mut root = config
        .as_object()
        .cloned()
        .ok_or_else(|| "Agent Identity auth_config must be a JSON object".to_string())?;
    let nested_key = ["agent_identity", "agentIdentity"]
        .into_iter()
        .find(|key| root.get(*key).and_then(Value::as_object).is_some());
    if let Some(nested_key) = nested_key {
        let nested = root
            .get_mut(nested_key)
            .and_then(Value::as_object_mut)
            .expect("Agent Identity nested config was checked as an object");
        nested.insert("task_id".to_string(), Value::String(task_id.clone()));
        nested.remove("taskId");
    }
    root.insert("task_id".to_string(), Value::String(task_id));
    root.remove("taskId");
    Ok(Value::Object(root))
}

fn agent_identity_refresh_error(message: impl Into<String>) -> LocalOAuthRefreshError {
    LocalOAuthRefreshError::InvalidResponse {
        provider_type: CODEX_AGENT_IDENTITY_PROVIDER_TYPE,
        message: message.into(),
    }
}

#[async_trait]
impl LocalOAuthRefreshAdapter for CodexAgentIdentityRefreshAdapter {
    fn provider_type(&self) -> &'static str {
        CODEX_AGENT_IDENTITY_PROVIDER_TYPE
    }

    fn supports(&self, transport: &GatewayProviderTransportSnapshot) -> bool {
        is_codex_agent_identity_transport(transport)
    }

    fn resolve_cached(
        &self,
        transport: &GatewayProviderTransportSnapshot,
        entry: &CachedOAuthEntry,
    ) -> Option<LocalResolvedOAuthRequestAuth> {
        let current_fingerprint = codex_agent_identity_transport_credential_fingerprint(transport)?;
        if entry.source_fingerprint.as_deref() != Some(current_fingerprint.as_str()) {
            return None;
        }
        let transport_config = Self::config_from_transport(transport)?;
        let entry_config = Self::config_from_entry(entry)?;
        let transport_task = agent_identity_credentials(&transport_config).ok()?.task_id;
        let entry_task = agent_identity_credentials(&entry_config).ok()?.task_id;
        if transport_task != entry_task {
            return None;
        }
        Self::resolve_from_config(&entry_config)
    }

    fn resolve_fenced_cached(
        &self,
        transport: &GatewayProviderTransportSnapshot,
        entry: &CachedOAuthEntry,
    ) -> Option<LocalResolvedOAuthRequestAuth> {
        self.resolve_cached(transport, entry)
    }

    fn resolve_refreshed(
        &self,
        transport: &GatewayProviderTransportSnapshot,
        entry: &CachedOAuthEntry,
    ) -> Option<LocalResolvedOAuthRequestAuth> {
        let current_fingerprint = codex_agent_identity_transport_credential_fingerprint(transport)?;
        if entry.source_fingerprint.as_deref() != Some(current_fingerprint.as_str()) {
            return None;
        }
        Self::config_from_entry(entry).and_then(|config| Self::resolve_from_config(&config))
    }

    fn resolve_without_refresh(
        &self,
        transport: &GatewayProviderTransportSnapshot,
    ) -> Option<LocalResolvedOAuthRequestAuth> {
        Self::config_from_transport(transport).and_then(|config| Self::resolve_from_config(&config))
    }

    fn should_refresh(
        &self,
        transport: &GatewayProviderTransportSnapshot,
        entry: Option<&CachedOAuthEntry>,
    ) -> bool {
        if !self.supports(transport) {
            return false;
        }
        Self::preferred_config(transport, entry)
            .and_then(|config| agent_identity_credentials(&config).ok())
            .is_some_and(|credentials| credentials.task_id.is_none())
    }

    fn refresh_fingerprint(
        &self,
        transport: &GatewayProviderTransportSnapshot,
        entry: Option<&CachedOAuthEntry>,
    ) -> Option<String> {
        codex_agent_identity_refresh_fingerprint(transport, entry)
    }

    fn cached_entry_from_transport(
        &self,
        transport: &GatewayProviderTransportSnapshot,
    ) -> Option<CachedOAuthEntry> {
        codex_agent_identity_cached_entry_from_transport(transport)
    }

    fn should_backoff_after_error(&self, error: &LocalOAuthRefreshError) -> bool {
        match error {
            LocalOAuthRefreshError::HttpStatus { status_code, .. } => {
                *status_code == 429 || *status_code >= 500
            }
            LocalOAuthRefreshError::Transport { .. }
            | LocalOAuthRefreshError::TransportMessage { .. }
            | LocalOAuthRefreshError::InvalidResponse { .. } => true,
        }
    }

    fn requires_distributed_refresh_lock(&self) -> bool {
        true
    }

    async fn refresh(
        &self,
        executor: &dyn LocalOAuthHttpExecutor,
        transport: &GatewayProviderTransportSnapshot,
        entry: Option<&CachedOAuthEntry>,
    ) -> Result<Option<CachedOAuthEntry>, LocalOAuthRefreshError> {
        let Some(config) = Self::preferred_config(transport, entry) else {
            return Ok(None);
        };
        let credentials =
            agent_identity_credentials(&config).map_err(agent_identity_refresh_error)?;
        let (timestamp, signature) = build_task_registration_signature(&credentials, Utc::now());
        let url = task_registration_url(&self.auth_api_base_url, credentials.runtime_id.as_str())
            .map_err(agent_identity_refresh_error)?;
        let request = LocalOAuthHttpRequest {
            request_id: CODEX_AGENT_IDENTITY_TASK_REGISTRATION_REQUEST_ID,
            method: reqwest::Method::POST,
            url,
            headers: BTreeMap::from([
                ("accept".to_string(), "application/json".to_string()),
                ("content-type".to_string(), "application/json".to_string()),
            ]),
            json_body: Some(json!({
                "timestamp": timestamp,
                "signature": signature,
            })),
            body_bytes: None,
        };
        let response = executor
            .execute(CODEX_AGENT_IDENTITY_PROVIDER_TYPE, transport, &request)
            .await?;
        if !(200..300).contains(&response.status_code) {
            return Err(LocalOAuthRefreshError::HttpStatus {
                provider_type: CODEX_AGENT_IDENTITY_PROVIDER_TYPE,
                status_code: response.status_code,
                // Registration responses can echo credential material. Keep the stored/logged
                // error deliberately generic instead of forwarding an upstream body excerpt.
                body_excerpt: format!(
                    "Agent Identity task registration returned HTTP {}",
                    response.status_code
                ),
            });
        }
        let task_id = task_id_from_registration_response(&credentials, response.body_text.as_str())
            .map_err(agent_identity_refresh_error)?;
        let config = with_agent_identity_task_id(&config, task_id.clone())
            .map_err(agent_identity_refresh_error)?;
        let updated_credentials =
            agent_identity_credentials(&config).map_err(agent_identity_refresh_error)?;
        let auth_header_value = build_agent_assertion(&updated_credentials, &task_id, Utc::now())
            .map_err(agent_identity_refresh_error)?;

        Ok(Some(CachedOAuthEntry {
            provider_type: CODEX_AGENT_IDENTITY_CACHED_ENTRY_PROVIDER_TYPE.to_string(),
            auth_header_name: AUTHORIZATION_HEADER.to_string(),
            auth_header_value,
            expires_at_unix_secs: None,
            metadata: Some(config),
            source_fingerprint: Some(agent_identity_credential_fingerprint_from_credentials(
                &updated_credentials,
            )),
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use aether_oauth::network::{
        OAuthHttpExecutor, OAuthHttpRequest, OAuthHttpResponse, OAuthNetworkContext,
    };
    use crypto_box::{aead::rand_core::OsRng, PublicKey};
    use ed25519_dalek::{pkcs8::EncodePrivateKey, Signature, Verifier};
    use serde_json::json;

    use super::{
        agent_identity_credentials, build_agent_assertion,
        codex_agent_identity_authorization_matches_transport,
        codex_agent_identity_cached_entry_from_transport,
        codex_agent_identity_config_refresh_fingerprint, codex_agent_identity_refresh_fingerprint,
        codex_agent_identity_transport_allows_task_rotation_from,
        create_codex_agent_identity_from_session_token_with_auth_api_base_url,
        decrypt_agent_task_id, is_codex_agent_identity_auth_config_value,
        is_codex_agent_identity_authorization, is_codex_agent_identity_invalid_task_response,
        register_codex_agent_identity_from_access_token_with_auth_api_base_url,
        task_id_from_registration_response, validate_codex_agent_identity_auth_config,
        with_agent_identity_task_id, CodexAgentIdentityEnrollmentError,
        CodexAgentIdentityRefreshAdapter, CodexAgentIdentityRegistrationOptions,
        CODEX_AGENT_IDENTITY_CACHED_ENTRY_PROVIDER_TYPE,
    };
    use crate::oauth_refresh::{
        LocalOAuthHttpExecutor, LocalOAuthHttpRequest, LocalOAuthHttpResponse,
        LocalOAuthRefreshAdapter, LocalOAuthRefreshCoordinator, LocalOAuthRefreshError,
        LocalResolvedOAuthRequestAuth,
    };
    use crate::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };
    use base64::{
        engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
        Engine as _,
    };
    use chrono::{TimeZone, Utc};
    use ed25519_dalek::SigningKey;
    use sha2::Digest;

    fn test_auth_config(task_id: Option<&str>) -> serde_json::Value {
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let private_key_der = signing_key
            .to_pkcs8_der()
            .expect("test key should encode as PKCS#8");
        let mut config = serde_json::Map::from_iter([
            ("provider_type".to_string(), json!("codex")),
            ("auth_mode".to_string(), json!("agentIdentity")),
            ("agent_runtime_id".to_string(), json!("runtime-test")),
            (
                "agent_private_key".to_string(),
                json!(STANDARD.encode(private_key_der.as_bytes())),
            ),
        ]);
        if let Some(task_id) = task_id {
            config.insert("task_id".to_string(), json!(task_id));
        }
        serde_json::Value::Object(config)
    }

    fn sample_transport(config: serde_json::Value) -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "Codex".to_string(),
                provider_type: "codex".to_string(),
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
                api_format: "openai:responses".to_string(),
                api_family: Some("openai".to_string()),
                endpoint_kind: Some("cli".to_string()),
                is_active: true,
                base_url: "https://chatgpt.com/backend-api/codex".to_string(),
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
                name: "Agent Identity".to_string(),
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
                decrypted_api_key: "__placeholder__".to_string(),
                decrypted_auth_config: Some(config.to_string()),
            },
        }
    }

    #[test]
    fn builds_verifiable_agent_assertion() {
        let config = test_auth_config(Some("task-test"));
        let credentials = agent_identity_credentials(&config).expect("credentials should parse");
        let now = Utc.with_ymd_and_hms(2030, 1, 2, 3, 4, 5).unwrap();

        let assertion =
            build_agent_assertion(&credentials, "task-test", now).expect("assertion should build");
        let encoded = assertion
            .strip_prefix("AgentAssertion ")
            .expect("assertion should have its scheme");
        let envelope: serde_json::Value = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(encoded)
                .expect("assertion should be URL-safe base64"),
        )
        .expect("assertion envelope should be JSON");

        assert_eq!(envelope["agent_runtime_id"], "runtime-test");
        assert_eq!(envelope["task_id"], "task-test");
        assert_eq!(envelope["timestamp"], "2030-01-02T03:04:05Z");
        let signature = Signature::from_slice(
            &STANDARD
                .decode(envelope["signature"].as_str().unwrap())
                .expect("signature should be base64"),
        )
        .expect("signature should be valid length");
        credentials
            .signing_key
            .verifying_key()
            .verify(b"runtime-test:task-test:2030-01-02T03:04:05Z", &signature)
            .expect("assertion signature should verify");
    }

    #[test]
    fn recognizes_agent_assertion_authorization_scheme_without_parsing_secrets() {
        assert!(is_codex_agent_identity_authorization(
            "AgentAssertion assertion-envelope"
        ));
        assert!(is_codex_agent_identity_authorization(
            "agentassertion\tassertion-envelope"
        ));
        assert!(!is_codex_agent_identity_authorization(
            "Bearer access-token"
        ));
        assert!(!is_codex_agent_identity_authorization("AgentAssertion"));
    }

    #[test]
    fn assertion_match_binds_runtime_task_and_signing_key_generation() {
        let config = test_auth_config(Some("task-test"));
        let credentials = agent_identity_credentials(&config).expect("credentials should parse");
        let assertion = build_agent_assertion(
            &credentials,
            "task-test",
            Utc.with_ymd_and_hms(2030, 1, 2, 3, 4, 5).unwrap(),
        )
        .expect("assertion should build");

        assert!(codex_agent_identity_authorization_matches_transport(
            &sample_transport(config.clone()),
            &assertion,
        ));
        assert!(!codex_agent_identity_authorization_matches_transport(
            &sample_transport(test_auth_config(Some("task-replaced"))),
            &assertion,
        ));

        let replacement_key = SigningKey::from_bytes(&[8u8; 32])
            .to_pkcs8_der()
            .expect("replacement key should encode");
        let mut replacement_config = config;
        replacement_config["agent_private_key"] =
            json!(STANDARD.encode(replacement_key.as_bytes()));
        assert!(!codex_agent_identity_authorization_matches_transport(
            &sample_transport(replacement_config),
            &assertion,
        ));
    }

    #[test]
    fn agent_identity_rejects_oversized_base64_before_decode() {
        let encoded_limit = super::maximum_base64_len_for_decoded_limit(
            super::MAX_AGENT_IDENTITY_ASSERTION_ENVELOPE_BYTES,
        );
        let assertion = format!("AgentAssertion {}", "A".repeat(encoded_limit + 1));
        assert!(!codex_agent_identity_authorization_matches_transport(
            &sample_transport(test_auth_config(Some("task-test"))),
            &assertion,
        ));

        let mut config = test_auth_config(Some("task-test"));
        let private_key_limit = super::maximum_base64_len_for_decoded_limit(
            super::MAX_AGENT_IDENTITY_PRIVATE_KEY_DER_BYTES,
        );
        config["agent_private_key"] = json!("A".repeat(private_key_limit + 1));
        assert!(agent_identity_credentials(&config).is_err());

        let credentials =
            agent_identity_credentials(&test_auth_config(None)).expect("credentials should parse");
        let encrypted_task_id_limit = super::maximum_base64_len_for_decoded_limit(
            super::MAX_AGENT_IDENTITY_ENCRYPTED_TASK_ID_BYTES,
        );
        assert!(
            decrypt_agent_task_id(&credentials, &"A".repeat(encrypted_task_id_limit + 1),).is_err()
        );
    }

    #[test]
    fn task_rotation_context_rejects_metadata_and_credential_replacement() {
        let initial = sample_transport(test_auth_config(Some("task-old")));
        let rotated = sample_transport(test_auth_config(Some("task-new")));
        assert!(codex_agent_identity_transport_allows_task_rotation_from(
            &initial, &rotated
        ));

        let mut metadata_rewrite = test_auth_config(Some("task-new"));
        metadata_rewrite["account_id"] = json!("account-replaced");
        assert!(!codex_agent_identity_transport_allows_task_rotation_from(
            &initial,
            &sample_transport(metadata_rewrite),
        ));

        let replacement_key = SigningKey::from_bytes(&[9u8; 32])
            .to_pkcs8_der()
            .expect("replacement key should encode");
        let mut credential_rewrite = test_auth_config(Some("task-new"));
        credential_rewrite["agent_private_key"] =
            json!(STANDARD.encode(replacement_key.as_bytes()));
        assert!(!codex_agent_identity_transport_allows_task_rotation_from(
            &initial,
            &sample_transport(credential_rewrite),
        ));
    }

    #[test]
    fn decrypts_sealed_task_registration_response() {
        let config = test_auth_config(None);
        let credentials = agent_identity_credentials(&config).expect("credentials should parse");
        let digest = sha2::Sha512::digest(credentials.signing_key.to_bytes());
        let mut curve_private_key = [0u8; 32];
        curve_private_key.copy_from_slice(&digest[..32]);
        let secret_key = crypto_box::SecretKey::from_bytes(curve_private_key);
        let public_key = PublicKey::from(&secret_key);
        let encrypted = public_key
            .seal(&mut OsRng, b"task-encrypted")
            .expect("sealed task should encrypt");

        assert_eq!(
            decrypt_agent_task_id(&credentials, &STANDARD.encode(encrypted))
                .expect("sealed task should decrypt"),
            "task-encrypted"
        );
    }

    #[test]
    fn accepts_nested_agent_identity_export_shape() {
        let nested = test_auth_config(Some("task-nested"));
        let config = json!({
            "provider_type": "codex",
            "auth_mode": "agentIdentity",
            "agent_identity": nested,
        });

        assert!(is_codex_agent_identity_auth_config_value(&config));
        assert_eq!(
            agent_identity_credentials(&config)
                .expect("nested credentials should parse")
                .task_id
                .as_deref(),
            Some("task-nested")
        );
    }

    #[test]
    fn synchronizes_replaced_task_id_across_nested_and_flat_import_fields() {
        let config = json!({
            "auth_mode": "agentIdentity",
            "taskId": "old-root-task",
            "agent_identity": {
                "agent_runtime_id": "runtime-test",
                "agent_private_key": "placeholder",
                "taskId": "old-nested-task"
            }
        });

        let updated =
            with_agent_identity_task_id(&config, "new-task".to_string()).expect("task updates");

        assert_eq!(updated["task_id"], "new-task");
        assert!(updated.get("taskId").is_none());
        assert_eq!(updated["agent_identity"]["task_id"], "new-task");
        assert!(updated["agent_identity"].get("taskId").is_none());
    }

    #[test]
    fn refresh_fingerprint_fences_task_generation_not_only_keypair() {
        let pending = test_auth_config(None);
        let registered = test_auth_config(Some("task-winner"));
        assert_ne!(
            codex_agent_identity_config_refresh_fingerprint(&pending),
            codex_agent_identity_config_refresh_fingerprint(&registered)
        );
    }

    #[derive(Clone)]
    struct RecordingExecutor {
        requests: Arc<Mutex<Vec<LocalOAuthHttpRequest>>>,
    }

    #[async_trait::async_trait]
    impl LocalOAuthHttpExecutor for RecordingExecutor {
        async fn execute(
            &self,
            _provider_type: &'static str,
            _transport: &GatewayProviderTransportSnapshot,
            request: &LocalOAuthHttpRequest,
        ) -> Result<LocalOAuthHttpResponse, LocalOAuthRefreshError> {
            self.requests
                .lock()
                .expect("recording lock should hold")
                .push(request.clone());
            Ok(LocalOAuthHttpResponse {
                status_code: 200,
                body_text: r#"{"task_id":"task-registered"}"#.to_string(),
            })
        }
    }

    #[derive(Clone)]
    struct RecordingEnrollmentExecutor {
        requests: Arc<Mutex<Vec<OAuthHttpRequest>>>,
        responses: Arc<Mutex<Vec<OAuthHttpResponse>>>,
    }

    #[async_trait::async_trait]
    impl OAuthHttpExecutor for RecordingEnrollmentExecutor {
        async fn execute(
            &self,
            request: OAuthHttpRequest,
        ) -> Result<OAuthHttpResponse, aether_oauth::core::OAuthError> {
            self.requests
                .lock()
                .expect("recording lock should hold")
                .push(request);
            let mut responses = self.responses.lock().expect("response lock should hold");
            if responses.is_empty() {
                return Err(aether_oauth::core::OAuthError::transport(
                    "missing mock response",
                ));
            }
            Ok(responses.remove(0))
        }
    }

    #[tokio::test]
    async fn enrolls_agent_identity_from_session_token_without_storing_it() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let executor = RecordingEnrollmentExecutor {
            requests: Arc::clone(&requests),
            responses: Arc::new(Mutex::new(vec![
                OAuthHttpResponse {
                    status_code: 200,
                    body_text: r#"{"agent_runtime_id":"runtime-enrolled"}"#.to_string(),
                    json_body: None,
                },
                OAuthHttpResponse {
                    status_code: 200,
                    body_text: r#"{"task_id":"task-enrolled"}"#.to_string(),
                    json_body: None,
                },
            ])),
        };

        let config = create_codex_agent_identity_from_session_token_with_auth_api_base_url(
            &executor,
            "session-token-for-test-only",
            OAuthNetworkContext::direct_identity(),
            "https://auth.test/api/accounts",
        )
        .await
        .expect("enrollment should succeed");

        validate_codex_agent_identity_auth_config(&serde_json::Value::Object(config.clone()))
            .expect("enrollment should return valid credentials");
        assert_eq!(
            config.get("agent_runtime_id"),
            Some(&json!("runtime-enrolled"))
        );
        assert_eq!(config.get("task_id"), Some(&json!("task-enrolled")));
        assert!(!config.contains_key("access_token"));
        assert!(!config.contains_key("refresh_token"));
        assert!(!config.contains_key("id_token"));
        assert!(!config
            .values()
            .any(|value| value.as_str() == Some("session-token-for-test-only")));

        let requests = requests.lock().expect("recording lock should hold");
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[0].url,
            "https://auth.test/api/accounts/v1/agent/register"
        );
        assert_eq!(
            requests[0].headers.get("authorization").map(String::as_str),
            Some("Bearer session-token-for-test-only")
        );
        assert_eq!(
            requests[0]
                .json_body
                .as_ref()
                .and_then(|body| body.get("abom"))
                .and_then(|abom| abom.get("agent_harness_id")),
            Some(&json!("codex-cli"))
        );
        assert_eq!(
            requests[0]
                .json_body
                .as_ref()
                .and_then(|body| body.get("capabilities")),
            Some(&json!(["responsesapi"]))
        );
        assert_eq!(
            requests[0]
                .json_body
                .as_ref()
                .and_then(|body| body.get("ttl")),
            Some(&json!(null))
        );
        assert!(requests[0]
            .json_body
            .as_ref()
            .and_then(|body| body.get("agent_public_key"))
            .and_then(serde_json::Value::as_str)
            .is_some_and(|key| key.starts_with("ssh-ed25519 ")));
        assert_eq!(
            requests[1].url,
            "https://auth.test/api/accounts/v1/agent/runtime-enrolled/task/register"
        );
        assert!(!requests[1].headers.contains_key("authorization"));
    }

    #[tokio::test]
    async fn register_only_returns_pending_config_without_registering_task() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let executor = RecordingEnrollmentExecutor {
            requests: Arc::clone(&requests),
            responses: Arc::new(Mutex::new(vec![OAuthHttpResponse {
                status_code: 200,
                body_text: r#"{"agent_runtime_id":"runtime-pending"}"#.to_string(),
                json_body: None,
            }])),
        };
        let config = register_codex_agent_identity_from_access_token_with_auth_api_base_url(
            &executor,
            "access-token-for-test-only",
            CodexAgentIdentityRegistrationOptions {
                is_fedramp_account: true,
            },
            OAuthNetworkContext::direct_identity(),
            "https://auth.test/api/accounts",
        )
        .await
        .expect("register-only enrollment should succeed");
        validate_codex_agent_identity_auth_config(&serde_json::Value::Object(config.clone()))
            .expect("pending config should be valid");
        assert_eq!(
            config.get("agent_runtime_id"),
            Some(&json!("runtime-pending"))
        );
        assert!(!config.contains_key("task_id"));
        let requests = requests.lock().expect("recording lock should hold");
        assert_eq!(requests.len(), 1);
        assert!(requests[0].url.ends_with("/v1/agent/register"));
        assert_eq!(
            requests[0]
                .headers
                .get("x-openai-fedramp")
                .map(String::as_str),
            Some("true")
        );
        assert_eq!(
            requests[0]
                .json_body
                .as_ref()
                .and_then(|body| body.get("abom"))
                .and_then(|abom| abom.get("running_location")),
            Some(&json!(format!("cli-{}", std::env::consts::OS)))
        );
    }

    #[tokio::test]
    async fn enrollment_error_does_not_echo_session_token_or_response_body() {
        let executor = RecordingEnrollmentExecutor {
            requests: Arc::new(Mutex::new(Vec::new())),
            responses: Arc::new(Mutex::new(vec![OAuthHttpResponse {
                status_code: 403,
                body_text: r#"{"detail":"session-token-for-test-only"}"#.to_string(),
                json_body: None,
            }])),
        };

        let error = create_codex_agent_identity_from_session_token_with_auth_api_base_url(
            &executor,
            "session-token-for-test-only",
            OAuthNetworkContext::direct_identity(),
            "https://auth.test/api/accounts",
        )
        .await
        .expect_err("rejected enrollment should fail");

        assert_eq!(
            error,
            CodexAgentIdentityEnrollmentError::RegistrationRejected {
                status_code: 403,
                reason: "：账户、工作区或出口区域未获服务端 Agent Identity 资格；该资格无法由 Aether 本地开通",
            }
        );
        assert!(!error.to_string().contains("session-token-for-test-only"));
        assert!(!error.to_string().contains("detail"));
    }

    #[tokio::test]
    async fn registers_missing_task_then_builds_a_fresh_assertion_from_cache() {
        let config = test_auth_config(None);
        let transport = sample_transport(config);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let executor = RecordingExecutor {
            requests: Arc::clone(&requests),
        };
        let adapter = CodexAgentIdentityRefreshAdapter::default()
            .with_auth_api_base_url_for_tests("https://auth.test/api/accounts");

        assert!(adapter.should_refresh(&transport, None));
        let entry = adapter
            .refresh(&executor, &transport, None)
            .await
            .expect("registration should succeed")
            .expect("registration should return a cache entry");
        assert_eq!(
            entry.provider_type,
            CODEX_AGENT_IDENTITY_CACHED_ENTRY_PROVIDER_TYPE
        );
        assert!(entry.auth_header_value.starts_with("AgentAssertion "));
        assert_eq!(
            entry
                .metadata
                .as_ref()
                .and_then(|value| value.get("task_id")),
            Some(&json!("task-registered"))
        );
        assert!(adapter.resolve_cached(&transport, &entry).is_none());
        let cached_auth = adapter
            .resolve_refreshed(&transport, &entry)
            .expect("new refresh result should create an assertion");
        assert!(matches!(
            cached_auth,
            LocalResolvedOAuthRequestAuth::Header { ref name, ref value }
                if name == "authorization" && value.starts_with("AgentAssertion ")
        ));

        let requests = requests.lock().expect("recording lock should hold");
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].request_id,
            super::CODEX_AGENT_IDENTITY_TASK_REGISTRATION_REQUEST_ID
        );
        assert_eq!(
            requests[0].url,
            "https://auth.test/api/accounts/v1/agent/runtime-test/task/register"
        );
        assert!(requests[0].json_body.as_ref().unwrap()["timestamp"]
            .as_str()
            .is_some());
        assert!(requests[0].json_body.as_ref().unwrap()["signature"]
            .as_str()
            .is_some());
    }

    #[tokio::test]
    async fn rejects_cached_task_after_agent_credential_rotation() {
        let transport = sample_transport(test_auth_config(None));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let adapter = CodexAgentIdentityRefreshAdapter::default()
            .with_auth_api_base_url_for_tests("https://auth.test/api/accounts");
        let entry = adapter
            .refresh(
                &RecordingExecutor {
                    requests: Arc::clone(&requests),
                },
                &transport,
                None,
            )
            .await
            .expect("registration should succeed")
            .expect("registration should return an entry");
        let mut rotated_config = test_auth_config(Some("task-rotated"));
        rotated_config["agent_runtime_id"] = json!("runtime-rotated");
        let rotated_transport = sample_transport(rotated_config);

        assert!(adapter.resolve_cached(&rotated_transport, &entry).is_none());
        assert!(adapter
            .resolve_without_refresh(&rotated_transport)
            .is_some());
    }

    #[tokio::test]
    async fn rejects_cached_task_after_remote_task_rotation() {
        let transport = sample_transport(test_auth_config(None));
        let adapter = CodexAgentIdentityRefreshAdapter::default()
            .with_auth_api_base_url_for_tests("https://auth.test/api/accounts");
        let entry = adapter
            .refresh(
                &RecordingExecutor {
                    requests: Arc::new(Mutex::new(Vec::new())),
                },
                &transport,
                None,
            )
            .await
            .expect("registration should succeed")
            .expect("registration should return an entry");
        let rotated_transport = sample_transport(test_auth_config(Some("task-new-winner")));

        assert!(adapter.resolve_cached(&rotated_transport, &entry).is_none());
        assert!(adapter
            .resolve_without_refresh(&rotated_transport)
            .is_some());
    }

    #[test]
    fn newer_transport_task_is_authoritative_over_stale_local_cache() {
        let stale_transport = sample_transport(test_auth_config(Some("task-stale")));
        let stale_entry = codex_agent_identity_cached_entry_from_transport(&stale_transport)
            .expect("stale transport should produce a cache entry");
        let current_config = test_auth_config(Some("task-current"));
        let current_transport = sample_transport(current_config.clone());

        assert_eq!(
            codex_agent_identity_refresh_fingerprint(&current_transport, Some(&stale_entry)),
            codex_agent_identity_config_refresh_fingerprint(&current_config)
        );
        assert!(CodexAgentIdentityRefreshAdapter::default()
            .resolve_fenced_cached(&current_transport, &stale_entry)
            .is_none());
    }

    #[tokio::test]
    async fn distributed_waiter_reuses_reloaded_transport_not_stale_cache() {
        let stale_transport = sample_transport(test_auth_config(Some("task-stale")));
        let stale_entry = codex_agent_identity_cached_entry_from_transport(&stale_transport)
            .expect("stale transport should produce a cache entry");
        let expected = codex_agent_identity_refresh_fingerprint(&stale_transport, None)
            .expect("stale transport should have a generation");
        let current_transport = sample_transport(test_auth_config(Some("task-current")));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let coordinator = LocalOAuthRefreshCoordinator::with_adapters_for_tests(vec![Arc::new(
            CodexAgentIdentityRefreshAdapter::default()
                .with_auth_api_base_url_for_tests("https://auth.test/api/accounts"),
        )]);
        coordinator
            .store_cached_entry(current_transport.key.id.as_str(), stale_entry)
            .await;

        let resolution = coordinator
            .force_refresh_with_result_fenced(
                &RecordingExecutor {
                    requests: Arc::clone(&requests),
                },
                &current_transport,
                None,
                None,
                Some(expected.as_str()),
            )
            .await
            .expect("waiter resolution should succeed")
            .expect("waiter should reuse the DB winner");

        assert!(resolution.reused_refresh);
        assert_eq!(
            resolution
                .refreshed_entry
                .as_ref()
                .and_then(|entry| entry.metadata.as_ref())
                .and_then(|config| config.get("task_id")),
            Some(&json!("task-current"))
        );
        assert!(requests
            .lock()
            .expect("recording lock should hold")
            .is_empty());
    }

    #[test]
    fn registration_response_accepts_encrypted_task_aliases() {
        let config = test_auth_config(Some("task-original"));
        let credentials = agent_identity_credentials(&config).expect("credentials should parse");

        assert_eq!(
            task_id_from_registration_response(&credentials, r#"{"taskId":"task-camel"}"#)
                .expect("camel task id should parse"),
            "task-camel"
        );
    }

    #[test]
    fn recognizes_only_agent_task_specific_unauthorized_responses() {
        assert!(is_codex_agent_identity_invalid_task_response(
            401,
            Some(r#"{"error": {"code": "invalid_task_id"}}"#),
        ));
        assert!(is_codex_agent_identity_invalid_task_response(
            401,
            Some("Agent task expired; register a new task"),
        ));
        assert!(!is_codex_agent_identity_invalid_task_response(
            401,
            Some(r#"{"error": {"code": "invalid_api_key"}}"#),
        ));
        assert!(!is_codex_agent_identity_invalid_task_response(
            403,
            Some(r#"{"error": {"code": "invalid_task_id"}}"#),
        ));
    }
}
