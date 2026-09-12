use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::outbound_request_policy::{
    ProviderOutboundRequestContext, ProviderOutboundRequestIdentityScope,
    ProviderOutboundRequestMutationScope, ProviderOutboundRequestPolicy,
    ProviderOutboundRequestPolicyReason, ProviderOutboundRequestPolicyResult,
};
use crate::snapshot::GatewayProviderTransportSnapshot;

pub const CODEX_FINGERPRINT_CONFIG_NAMESPACE: &str = "codex";
pub const CODEX_FINGERPRINT_ENABLED_CONFIG_KEY: &str = "fingerprint_convergence_enabled";

pub type CodexFingerprintConvergenceContext = ProviderOutboundRequestContext;

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodexConvergedFingerprint {
    installation_id: String,
    session_id: String,
    thread_id: String,
    turn_id: String,
    window_id: String,
    turn_started_at_unix_ms: u64,
    prompt_cache_key: Option<String>,
}

pub fn codex_fingerprint_convergence_enabled(
    provider_type: &str,
    provider_config: Option<&Value>,
) -> bool {
    provider_type.trim().eq_ignore_ascii_case("codex")
        && provider_config
            .and_then(Value::as_object)
            .and_then(|config| config.get(CODEX_FINGERPRINT_CONFIG_NAMESPACE))
            .and_then(Value::as_object)
            .and_then(|config| config.get(CODEX_FINGERPRINT_ENABLED_CONFIG_KEY))
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

pub fn apply_codex_fingerprint_convergence(
    transport: &GatewayProviderTransportSnapshot,
    provider_api_format: &str,
    original_client_session_id: Option<&str>,
    provider_request_headers: &mut BTreeMap<String, String>,
    provider_request_body: &mut Value,
) -> bool {
    let mut context =
        CodexFingerprintConvergenceContext::new(Uuid::now_v7().to_string(), current_unix_millis());
    if let Some(original_client_session_id) = original_client_session_id {
        context = context.with_original_client_session_id(original_client_session_id);
    }
    apply_codex_fingerprint_convergence_with_context(
        transport,
        provider_api_format,
        &context,
        provider_request_headers,
        provider_request_body,
    )
}

pub fn apply_codex_fingerprint_convergence_with_context(
    transport: &GatewayProviderTransportSnapshot,
    provider_api_format: &str,
    context: &CodexFingerprintConvergenceContext,
    provider_request_headers: &mut BTreeMap<String, String>,
    provider_request_body: &mut Value,
) -> bool {
    apply_codex_fingerprint_convergence_policy(
        transport,
        provider_api_format,
        context,
        provider_request_headers,
        provider_request_body,
    )
    .was_applied()
}

pub(crate) fn apply_codex_fingerprint_convergence_policy(
    transport: &GatewayProviderTransportSnapshot,
    provider_api_format: &str,
    context: &ProviderOutboundRequestContext,
    provider_request_headers: &mut BTreeMap<String, String>,
    provider_request_body: &mut Value,
) -> ProviderOutboundRequestPolicyResult {
    let policy = ProviderOutboundRequestPolicy::CodexFingerprintConvergence;
    if !transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("codex")
    {
        return ProviderOutboundRequestPolicyResult::skipped(
            policy,
            ProviderOutboundRequestPolicyReason::ProviderTypeMismatch,
        );
    }
    if crate::agent_identity::is_codex_agent_identity_transport(transport) {
        return ProviderOutboundRequestPolicyResult::skipped(
            policy,
            ProviderOutboundRequestPolicyReason::AgentIdentityExcluded,
        );
    }

    let is_responses = aether_ai_formats::is_openai_responses_format(provider_api_format);
    let is_live = aether_ai_formats::api_format_alias_matches(provider_api_format, "codex:live");
    if !is_responses && !is_live {
        return ProviderOutboundRequestPolicyResult::skipped(
            policy,
            ProviderOutboundRequestPolicyReason::UnsupportedApiFormat,
        );
    }
    if is_responses
        && aether_ai_formats::openai_responses_request_operation(
            provider_api_format,
            provider_request_body,
        ) == Some(aether_ai_formats::OPENAI_RESPONSES_OPERATION_COMPACT)
    {
        return ProviderOutboundRequestPolicyResult::skipped(
            policy,
            ProviderOutboundRequestPolicyReason::CompactOperationExcluded,
        );
    }
    if !codex_fingerprint_convergence_enabled(
        transport.provider.provider_type.as_str(),
        transport.provider.config.as_ref(),
    ) {
        return ProviderOutboundRequestPolicyResult::skipped(
            policy,
            ProviderOutboundRequestPolicyReason::Disabled,
        );
    }
    if !provider_request_body.is_object() {
        return ProviderOutboundRequestPolicyResult::skipped(
            policy,
            ProviderOutboundRequestPolicyReason::RequestBodyNotObject,
        );
    }

    let auth_identity = aether_ai_formats::parse_codex_auth_identity(
        transport.key.decrypted_auth_config.as_deref(),
    );
    let (account_seed, identity_scope) =
        resolve_codex_account_seed_with_scope(&auth_identity, transport.key.id.as_str());
    // Only namespace a cache key that survived all provider-body conversion and
    // routing rules. The client-side value in `context` is a retry signal, not
    // permission to resurrect a field that the terminal body deliberately
    // removed.
    let effective_prompt_cache_key = provider_request_body
        .as_object()
        .and_then(|body| body.get("prompt_cache_key"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let fingerprint = resolve_converged_fingerprint_with_prompt_cache(
        account_seed.as_str(),
        context,
        effective_prompt_cache_key,
    );

    apply_converged_headers(provider_request_headers, &fingerprint);
    // Live uses the converged identity on the WebSocket/call-control headers.
    // Its event/session payload is an independent opaque protocol and must not
    // receive Responses-only `client_metadata` fields.
    if is_responses {
        apply_converged_client_metadata(provider_request_body, &fingerprint);
    }
    ProviderOutboundRequestPolicyResult::applied(
        policy,
        if is_responses {
            ProviderOutboundRequestMutationScope::HeadersAndBody
        } else {
            ProviderOutboundRequestMutationScope::Headers
        },
        identity_scope,
    )
}

#[cfg(test)]
fn resolve_converged_fingerprint(
    account_seed: &str,
    context: &CodexFingerprintConvergenceContext,
) -> CodexConvergedFingerprint {
    resolve_converged_fingerprint_with_prompt_cache(account_seed, context, None)
}

fn resolve_converged_fingerprint_with_prompt_cache(
    account_seed: &str,
    context: &CodexFingerprintConvergenceContext,
    effective_prompt_cache_key: Option<&str>,
) -> CodexConvergedFingerprint {
    let installation_id =
        derive_stable_uuid_v4(&format!("aether:codex-installation-id:v1:{account_seed}"));
    let session_id = derive_stable_uuid_v4(&format!("aether:codex-session-id:v1:{account_seed}"));
    let original_client_session_id = context
        .original_client_session_id()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let thread_id = original_client_session_id
        .map(|client_session_id| {
            derive_stable_uuid_v4(&format!(
                "aether:codex-thread-id:v1:{account_seed}:{client_session_id}"
            ))
        })
        .unwrap_or_else(|| session_id.clone());
    let window_id = format!("{thread_id}:0");
    let turn_identity = context
        .original_turn_id()
        .map(|turn_id| ("original", turn_id))
        .unwrap_or_else(|| ("logical", context.logical_turn_id()));
    let turn_id = derive_stable_uuid_v7(
        context.turn_started_at_unix_ms(),
        &format!(
            "aether:codex-turn-id:v1\0{account_seed}\0{}\0{}",
            turn_identity.0, turn_identity.1
        ),
    );
    let prompt_cache_key = context
        .original_prompt_cache_key()
        .and(effective_prompt_cache_key)
        .map(|effective| {
            Uuid::new_v5(
                &Uuid::NAMESPACE_URL,
                format!("aether:codex-prompt-cache-key:v1\0{account_seed}\0{effective}").as_bytes(),
            )
            .to_string()
        });

    CodexConvergedFingerprint {
        installation_id,
        session_id,
        thread_id,
        turn_id,
        window_id,
        turn_started_at_unix_ms: context.turn_started_at_unix_ms(),
        prompt_cache_key,
    }
}

fn current_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

fn normalized_identity_part(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

#[cfg(test)]
fn resolve_codex_account_seed(
    identity: &aether_ai_formats::CodexAuthIdentity,
    fallback_key_id: &str,
) -> String {
    resolve_codex_account_seed_with_scope(identity, fallback_key_id).0
}

fn resolve_codex_account_seed_with_scope(
    identity: &aether_ai_formats::CodexAuthIdentity,
    fallback_key_id: &str,
) -> (String, ProviderOutboundRequestIdentityScope) {
    if let Some(fingerprint) =
        normalized_identity_part(identity.codex_identity_fingerprint.as_deref())
    {
        return (
            format!("persisted:v1:{fingerprint}"),
            ProviderOutboundRequestIdentityScope::PersistedFingerprint,
        );
    }

    let account = normalized_identity_part(identity.account_id.as_deref());
    let member = normalized_identity_part(identity.account_user_id.as_deref())
        .or_else(|| normalized_identity_part(identity.user_id.as_deref()))
        .or_else(|| normalized_identity_part(identity.email.as_deref()));
    if let Some(fingerprint) = aether_oauth::provider::providers::derive_codex_identity_fingerprint(
        account.as_deref(),
        member.as_deref(),
        None,
        None,
    ) {
        let scope = if account.is_some() {
            ProviderOutboundRequestIdentityScope::AccountMember
        } else {
            ProviderOutboundRequestIdentityScope::Member
        };
        return (format!("persisted:v1:{fingerprint}"), scope);
    }

    match (account, member) {
        (Some(account), Some(member)) => (
            format!("account-member:v1:{account}\0{member}"),
            ProviderOutboundRequestIdentityScope::AccountMember,
        ),
        (None, Some(member)) => (
            format!("member:v1:{member}"),
            ProviderOutboundRequestIdentityScope::Member,
        ),
        (Some(account), None) => (
            format!("account:v1:{account}"),
            ProviderOutboundRequestIdentityScope::Account,
        ),
        (None, None) => (
            format!("key:v1:{}", fallback_key_id.trim()),
            ProviderOutboundRequestIdentityScope::Key,
        ),
    }
}

fn derive_stable_uuid_v4(seed: &str) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes).to_string()
}

fn derive_stable_uuid_v7(timestamp_ms: u64, seed: &str) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    let mut bytes = [0_u8; 16];
    let timestamp_bytes = timestamp_ms.min(0x0000_ffff_ffff_ffff).to_be_bytes();
    bytes[..6].copy_from_slice(&timestamp_bytes[2..]);
    bytes[6..].copy_from_slice(&digest[..10]);
    bytes[6] = (bytes[6] & 0x0f) | 0x70;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes).to_string()
}

fn apply_converged_headers(
    headers: &mut BTreeMap<String, String>,
    fingerprint: &CodexConvergedFingerprint,
) {
    set_header(
        headers,
        "x-codex-installation-id",
        fingerprint.installation_id.clone(),
    );
    set_header(headers, "x-codex-window-id", fingerprint.window_id.clone());
    set_header(
        headers,
        "x-client-request-id",
        fingerprint.thread_id.clone(),
    );
    set_header(headers, "session-id", fingerprint.session_id.clone());
    set_header(headers, "session_id", fingerprint.session_id.clone());
    set_header(headers, "thread-id", fingerprint.thread_id.clone());
    // Codex Live/Realtime uses `x-session-id` for the thread-scoped session
    // identity on the WebSocket upgrade request. Keep it aligned with the
    // converged thread identity instead of the account-scoped session value.
    set_header(headers, "x-session-id", fingerprint.thread_id.clone());
    rewrite_header_turn_metadata(headers, fingerprint);
}

fn apply_converged_client_metadata(body: &mut Value, fingerprint: &CodexConvergedFingerprint) {
    let Some(body) = body.as_object_mut() else {
        return;
    };
    if let Some(prompt_cache_key) = fingerprint.prompt_cache_key.as_ref() {
        body.insert(
            "prompt_cache_key".to_string(),
            Value::String(prompt_cache_key.clone()),
        );
    }
    let metadata = body
        .entry("client_metadata".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !metadata.is_object() {
        *metadata = Value::Object(Map::new());
    }
    let Some(metadata) = metadata.as_object_mut() else {
        return;
    };

    metadata.insert(
        "x-codex-installation-id".to_string(),
        Value::String(fingerprint.installation_id.clone()),
    );
    metadata.insert(
        "session_id".to_string(),
        Value::String(fingerprint.session_id.clone()),
    );
    metadata.insert(
        "thread_id".to_string(),
        Value::String(fingerprint.thread_id.clone()),
    );
    metadata.insert(
        "turn_id".to_string(),
        Value::String(fingerprint.turn_id.clone()),
    );
    metadata.insert(
        "x-codex-window-id".to_string(),
        Value::String(fingerprint.window_id.clone()),
    );
    rewrite_embedded_turn_metadata(metadata, fingerprint);
}

fn rewrite_header_turn_metadata(
    headers: &mut BTreeMap<String, String>,
    fingerprint: &CodexConvergedFingerprint,
) {
    let Some((name, raw)) = find_header(headers, "x-codex-turn-metadata") else {
        return;
    };
    let Ok(mut metadata) = serde_json::from_str::<Map<String, Value>>(&raw) else {
        return;
    };
    apply_turn_metadata_fields(&mut metadata, fingerprint);
    let Ok(rebuilt) = serde_json::to_string(&metadata) else {
        return;
    };
    headers.remove(&name);
    headers.insert("x-codex-turn-metadata".to_string(), rebuilt);
}

fn rewrite_embedded_turn_metadata(
    metadata: &mut Map<String, Value>,
    fingerprint: &CodexConvergedFingerprint,
) {
    let Some(turn_metadata) = metadata.get_mut("x-codex-turn-metadata") else {
        return;
    };
    match turn_metadata {
        Value::Object(turn_metadata) => apply_turn_metadata_fields(turn_metadata, fingerprint),
        Value::String(raw) => {
            let Ok(mut parsed) = serde_json::from_str::<Map<String, Value>>(raw) else {
                return;
            };
            apply_turn_metadata_fields(&mut parsed, fingerprint);
            let Ok(rebuilt) = serde_json::to_string(&parsed) else {
                return;
            };
            *raw = rebuilt;
        }
        _ => {}
    }
}

fn apply_turn_metadata_fields(
    metadata: &mut Map<String, Value>,
    fingerprint: &CodexConvergedFingerprint,
) {
    metadata.insert(
        "installation_id".to_string(),
        Value::String(fingerprint.installation_id.clone()),
    );
    metadata.insert(
        "session_id".to_string(),
        Value::String(fingerprint.session_id.clone()),
    );
    metadata.insert(
        "thread_id".to_string(),
        Value::String(fingerprint.thread_id.clone()),
    );
    metadata.insert(
        "turn_id".to_string(),
        Value::String(fingerprint.turn_id.clone()),
    );
    metadata.insert(
        "window_id".to_string(),
        Value::String(fingerprint.window_id.clone()),
    );
    metadata.insert(
        "turn_started_at_unix_ms".to_string(),
        Value::from(fingerprint.turn_started_at_unix_ms),
    );
    if let Some(prompt_cache_key) = fingerprint.prompt_cache_key.as_ref() {
        metadata.insert(
            "prompt_cache_key".to_string(),
            Value::String(prompt_cache_key.clone()),
        );
    }
}

fn set_header(headers: &mut BTreeMap<String, String>, name: &str, value: String) {
    let matching_names = headers
        .keys()
        .filter(|candidate| candidate.eq_ignore_ascii_case(name))
        .cloned()
        .collect::<Vec<_>>();
    for matching_name in matching_names {
        headers.remove(&matching_name);
    }
    headers.insert(name.to_string(), value);
}

fn find_header(headers: &BTreeMap<String, String>, name: &str) -> Option<(String, String)> {
    headers
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
        .map(|(name, value)| (name.clone(), value.clone()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider,
    };

    fn sample_transport() -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "Codex".to_string(),
                provider_type: "codex".to_string(),
                website: None,
                is_active: true,
                keep_priority_on_conversion: false,
                enable_format_conversion: true,
                concurrent_limit: None,
                max_retries: None,
                proxy: None,
                request_timeout_secs: None,
                stream_first_byte_timeout_secs: None,
                config: Some(json!({
                    "codex": {"fingerprint_convergence_enabled": true},
                    "unrelated": {"kept": true}
                })),
            },
            endpoint: GatewayProviderTransportEndpoint {
                id: "endpoint-1".to_string(),
                provider_id: "provider-1".to_string(),
                api_format: "openai:responses".to_string(),
                api_family: None,
                endpoint_kind: None,
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
                name: "OAuth".to_string(),
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
                decrypted_api_key: "access-token".to_string(),
                decrypted_auth_config: Some(json!({"account_id": "account-1"}).to_string()),
            },
        }
    }

    #[test]
    fn provider_config_switch_is_opt_in_and_codex_only() {
        assert!(!codex_fingerprint_convergence_enabled("codex", None));
        assert!(!codex_fingerprint_convergence_enabled(
            "codex",
            Some(&json!({"codex": {"fingerprint_convergence_enabled": false}}))
        ));
        assert!(codex_fingerprint_convergence_enabled(
            "CODEX",
            Some(&json!({"codex": {"fingerprint_convergence_enabled": true}}))
        ));
        assert!(!codex_fingerprint_convergence_enabled(
            "openai",
            Some(&json!({"codex": {"fingerprint_convergence_enabled": true}}))
        ));
    }

    #[test]
    fn convergence_rewrites_headers_and_body_with_one_identity_set() {
        let transport = sample_transport();
        let mut headers = BTreeMap::from([
            ("Session-Id".to_string(), "client-session".to_string()),
            (
                "X-Session-Id".to_string(),
                "client-live-session".to_string(),
            ),
            (
                "x-codex-turn-metadata".to_string(),
                json!({
                    "installation_id": "client-installation",
                    "session_id": "client-session",
                    "thread_source": "cli"
                })
                .to_string(),
            ),
        ]);
        let mut body = json!({
            "model": "gpt-5.4",
            "client_metadata": {
                "session_id": "client-session",
                "x-codex-turn-metadata": json!({
                    "installation_id": "client-installation",
                    "sandbox": "workspace-write"
                }).to_string()
            }
        });

        assert!(apply_codex_fingerprint_convergence(
            &transport,
            "openai:responses",
            Some("client-session"),
            &mut headers,
            &mut body,
        ));

        let session_id = headers.get("session-id").expect("session header");
        let thread_id = headers.get("thread-id").expect("thread header");
        let installation_id = headers
            .get("x-codex-installation-id")
            .expect("installation header");
        assert_eq!(
            Uuid::parse_str(session_id)
                .expect("session UUID")
                .get_version_num(),
            4
        );
        assert_eq!(
            Uuid::parse_str(thread_id)
                .expect("thread UUID")
                .get_version_num(),
            4
        );
        assert_eq!(
            Uuid::parse_str(installation_id)
                .expect("installation UUID")
                .get_version_num(),
            4
        );
        assert_eq!(headers["session_id"], *session_id);
        assert_eq!(headers["x-client-request-id"], *thread_id);
        assert_eq!(headers["x-session-id"], *thread_id);
        assert_eq!(headers["x-codex-window-id"], format!("{thread_id}:0"));
        assert_eq!(
            headers
                .keys()
                .filter(|name| name.eq_ignore_ascii_case("x-session-id"))
                .count(),
            1
        );
        assert_eq!(body["client_metadata"]["session_id"], *session_id);
        assert_eq!(body["client_metadata"]["thread_id"], *thread_id);
        assert_eq!(
            body["client_metadata"]["x-codex-installation-id"],
            *installation_id
        );

        let header_metadata: Value =
            serde_json::from_str(&headers["x-codex-turn-metadata"]).expect("header metadata");
        let body_metadata: Value = serde_json::from_str(
            body["client_metadata"]["x-codex-turn-metadata"]
                .as_str()
                .expect("embedded metadata"),
        )
        .expect("body metadata");
        assert_eq!(
            header_metadata["turn_id"],
            body["client_metadata"]["turn_id"]
        );
        assert_eq!(body_metadata["turn_id"], body["client_metadata"]["turn_id"]);
        assert_eq!(header_metadata["thread_source"], "cli");
        assert_eq!(body_metadata["sandbox"], "workspace-write");
        assert_eq!(
            Uuid::parse_str(
                body["client_metadata"]["turn_id"]
                    .as_str()
                    .expect("turn id")
            )
            .expect("turn UUID")
            .get_version_num(),
            7
        );
    }

    #[test]
    fn object_form_embedded_turn_metadata_is_rewritten_with_the_same_identity() {
        let transport = sample_transport();
        let context = CodexFingerprintConvergenceContext::new("logical-turn-1", 1_700_000_000_123)
            .with_original_turn_id("client-turn-1")
            .with_original_client_session_id("client-session-1")
            .with_original_prompt_cache_key("client-cache-1");
        let mut headers = BTreeMap::new();
        let mut body = json!({
            "model": "gpt-5.4",
            "client_metadata": {
                "x-codex-turn-metadata": {
                    "installation_id": "old-installation",
                    "session_id": "old-session",
                    "thread_id": "old-thread",
                    "turn_id": "old-turn",
                    "window_id": "old-window",
                    "custom": "preserved"
                }
            }
        });

        assert!(apply_codex_fingerprint_convergence_with_context(
            &transport,
            "openai:responses",
            &context,
            &mut headers,
            &mut body,
        ));

        let embedded = &body["client_metadata"]["x-codex-turn-metadata"];
        assert!(embedded.is_object());
        assert_eq!(embedded["custom"], "preserved");
        assert_eq!(
            embedded["installation_id"],
            body["client_metadata"]["x-codex-installation-id"]
        );
        assert_eq!(
            embedded["session_id"],
            body["client_metadata"]["session_id"]
        );
        assert_eq!(embedded["thread_id"], body["client_metadata"]["thread_id"]);
        assert_eq!(embedded["turn_id"], body["client_metadata"]["turn_id"]);
        assert_eq!(
            embedded["window_id"],
            body["client_metadata"]["x-codex-window-id"]
        );
        assert_eq!(embedded["prompt_cache_key"], body["prompt_cache_key"]);
        assert_eq!(
            embedded["turn_started_at_unix_ms"],
            context.turn_started_at_unix_ms()
        );
    }

    #[test]
    fn stable_account_identity_client_thread_and_logical_turn_are_deterministic() {
        let context = CodexFingerprintConvergenceContext::new("logical-turn-1", 1_700_000_000_123)
            .with_original_client_session_id("client-a");
        let other_turn_context =
            CodexFingerprintConvergenceContext::new("logical-turn-2", 1_700_000_000_124)
                .with_original_client_session_id("client-a");
        let other_client_context = context.clone().with_original_client_session_id("client-b");
        let first = resolve_converged_fingerprint("account-1", &context);
        let second = resolve_converged_fingerprint("account-1", &context);
        let other_turn = resolve_converged_fingerprint("account-1", &other_turn_context);
        let other_client = resolve_converged_fingerprint("account-1", &other_client_context);
        let other_account = resolve_converged_fingerprint("account-2", &context);

        assert_eq!(first.installation_id, second.installation_id);
        assert_eq!(first.session_id, second.session_id);
        assert_eq!(first.thread_id, second.thread_id);
        assert_eq!(first.turn_id, second.turn_id);
        assert_eq!(first.turn_started_at_unix_ms, 1_700_000_000_123);
        assert_ne!(first.turn_id, other_turn.turn_id);
        assert_ne!(first.thread_id, other_client.thread_id);
        assert_eq!(first.session_id, other_client.session_id);
        assert_ne!(first.installation_id, other_account.installation_id);
        assert_ne!(first.turn_id, other_account.turn_id);
        assert_eq!(
            Uuid::parse_str(&first.turn_id)
                .expect("turn UUID")
                .get_version_num(),
            7
        );
        assert_eq!(
            first.turn_id.replace('-', "")[..12],
            format!("{:012x}", context.turn_started_at_unix_ms())
        );
    }

    #[test]
    fn legacy_wrapper_keeps_a_fresh_turn_and_does_not_namespace_prompt_cache() {
        let transport = sample_transport();
        let mut first_headers = BTreeMap::new();
        let mut second_headers = BTreeMap::new();
        let mut first_body = json!({
            "model": "gpt-5.4",
            "prompt_cache_key": "existing-cache"
        });
        let mut second_body = first_body.clone();

        assert!(apply_codex_fingerprint_convergence(
            &transport,
            "openai:responses",
            Some("client-session"),
            &mut first_headers,
            &mut first_body,
        ));
        assert!(apply_codex_fingerprint_convergence(
            &transport,
            "openai:responses",
            Some("client-session"),
            &mut second_headers,
            &mut second_body,
        ));

        assert_ne!(
            first_body["client_metadata"]["turn_id"],
            second_body["client_metadata"]["turn_id"]
        );
        assert_eq!(first_body["prompt_cache_key"], "existing-cache");
        assert_eq!(second_body["prompt_cache_key"], "existing-cache");
    }

    #[test]
    fn convergence_context_reuses_original_turn_time_and_namespaced_prompt_cache_key() {
        let mut transport = sample_transport();
        transport.key.decrypted_auth_config = Some(
            json!({
                "account_id": "workspace-1",
                "account_user_id": "member-1",
                "codex_identity_fingerprint": "codex-persisted-fingerprint:v1:member-1"
            })
            .to_string(),
        );
        let context =
            CodexFingerprintConvergenceContext::new("logical-attempt-1", 1_700_000_000_123)
                .with_original_turn_id("client-turn-1")
                .with_original_client_session_id("client-session-1")
                .with_original_prompt_cache_key("client-cache-1");
        let original_headers = BTreeMap::from([(
            "x-codex-turn-metadata".to_string(),
            json!({
                "turn_id": "client-turn-1",
                "prompt_cache_key": "client-cache-1"
            })
            .to_string(),
        )]);
        let original_body = json!({
            "model": "gpt-5.4",
            "prompt_cache_key": "already-adapted-cache",
            "client_metadata": {
                "x-codex-turn-metadata": json!({
                    "turn_id": "client-turn-1",
                    "prompt_cache_key": "client-cache-1"
                }).to_string()
            }
        });

        let mut first_headers = original_headers.clone();
        let mut first_body = original_body.clone();
        let mut retried_headers = original_headers;
        let mut retried_body = original_body;
        assert!(apply_codex_fingerprint_convergence_with_context(
            &transport,
            "openai:responses",
            &context,
            &mut first_headers,
            &mut first_body,
        ));
        assert!(apply_codex_fingerprint_convergence_with_context(
            &transport,
            "openai:responses",
            &context,
            &mut retried_headers,
            &mut retried_body,
        ));

        assert_eq!(first_headers, retried_headers);
        assert_eq!(first_body, retried_body);
        assert_eq!(context.logical_turn_id(), "logical-attempt-1");
        assert_eq!(context.original_turn_id(), Some("client-turn-1"));
        assert_eq!(
            context.original_client_session_id(),
            Some("client-session-1")
        );
        assert_eq!(context.original_prompt_cache_key(), Some("client-cache-1"));
        assert_eq!(context.turn_started_at_unix_ms(), 1_700_000_000_123);

        let prompt_cache_key = first_body["prompt_cache_key"]
            .as_str()
            .expect("prompt cache key");
        assert_ne!(prompt_cache_key, "client-cache-1");
        assert_ne!(prompt_cache_key, "already-adapted-cache");
        assert_eq!(
            Uuid::parse_str(prompt_cache_key)
                .expect("prompt cache UUID")
                .get_version_num(),
            5
        );
        let header_metadata: Value =
            serde_json::from_str(&first_headers["x-codex-turn-metadata"]).expect("header metadata");
        let body_metadata: Value = serde_json::from_str(
            first_body["client_metadata"]["x-codex-turn-metadata"]
                .as_str()
                .expect("body metadata"),
        )
        .expect("body metadata json");
        assert_eq!(header_metadata["prompt_cache_key"], prompt_cache_key);
        assert_eq!(body_metadata["prompt_cache_key"], prompt_cache_key);
        assert_eq!(
            header_metadata["turn_started_at_unix_ms"],
            1_700_000_000_123_u64
        );
        assert_eq!(
            body_metadata["turn_started_at_unix_ms"],
            1_700_000_000_123_u64
        );

        let same_original_turn = context.clone().with_original_turn_id("client-turn-1");
        let changed_logical_turn =
            CodexFingerprintConvergenceContext::new("logical-attempt-2", 1_700_000_000_123)
                .with_original_turn_id("client-turn-1");
        assert_eq!(
            resolve_converged_fingerprint("account-1", &same_original_turn).turn_id,
            resolve_converged_fingerprint("account-1", &changed_logical_turn).turn_id
        );
    }

    #[test]
    fn convergence_does_not_resurrect_a_removed_prompt_cache_key() {
        let transport = sample_transport();
        let context = CodexFingerprintConvergenceContext::new("logical-turn", 1_700_000_000_123)
            .with_original_prompt_cache_key("client-cache");
        let mut body = json!({
            "model": "gpt-5.4",
            "client_metadata": {}
        });
        let mut headers = BTreeMap::new();

        assert!(apply_codex_fingerprint_convergence_with_context(
            &transport,
            "openai:responses",
            &context,
            &mut headers,
            &mut body,
        ));

        assert!(body.get("prompt_cache_key").is_none());
        assert!(body["client_metadata"].get("prompt_cache_key").is_none());
    }

    #[test]
    fn persisted_or_canonical_member_identity_drives_the_account_seed() {
        let persisted_a = aether_ai_formats::parse_codex_auth_identity(Some(
            r#"{"account_id":"workspace-a","email":"old@example.com","codex_identity_fingerprint":"Stable-Member"}"#,
        ));
        let persisted_b = aether_ai_formats::parse_codex_auth_identity(Some(
            r#"{"account_id":"workspace-b","email":"new@example.com","codex_identity_fingerprint":"stable-member"}"#,
        ));
        assert_eq!(
            resolve_codex_account_seed(&persisted_a, "key-a"),
            resolve_codex_account_seed(&persisted_b, "key-b")
        );

        let canonical_a = aether_ai_formats::parse_codex_auth_identity(Some(
            r#"{"account_id":"Workspace-1","account_user_id":"Member-1","email":"old@example.com"}"#,
        ));
        let canonical_b = aether_ai_formats::parse_codex_auth_identity(Some(
            r#"{"account_id":"workspace-1","account_user_id":"member-1","email":"new@example.com"}"#,
        ));
        let other_member = aether_ai_formats::parse_codex_auth_identity(Some(
            r#"{"account_id":"workspace-1","account_user_id":"member-2"}"#,
        ));
        assert_eq!(
            resolve_codex_account_seed(&canonical_a, "key-a"),
            resolve_codex_account_seed(&canonical_b, "key-b")
        );
        assert_ne!(
            resolve_codex_account_seed(&canonical_a, "key-a"),
            resolve_codex_account_seed(&other_member, "key-a")
        );

        let derived_fingerprint =
            aether_oauth::provider::providers::derive_codex_identity_fingerprint(
                canonical_a.account_id.as_deref(),
                canonical_a.account_user_id.as_deref(),
                canonical_a.user_id.as_deref(),
                canonical_a.email.as_deref(),
            )
            .expect("canonical member fingerprint");
        let after_first_refresh = aether_ai_formats::parse_codex_auth_identity(Some(
            &json!({
                "account_id": "Workspace-1",
                "account_user_id": "Member-1",
                "email": "old@example.com",
                "codex_identity_fingerprint": derived_fingerprint
            })
            .to_string(),
        ));
        let legacy_seed = resolve_codex_account_seed(&canonical_a, "key-a");
        let refreshed_seed = resolve_codex_account_seed(&after_first_refresh, "key-a");
        assert_eq!(legacy_seed, refreshed_seed);

        let context = CodexFingerprintConvergenceContext::new("logical-turn-1", 1_700_000_000_123)
            .with_original_client_session_id("client-session-1")
            .with_original_prompt_cache_key("client-cache-1");
        assert_eq!(
            resolve_converged_fingerprint(&legacy_seed, &context),
            resolve_converged_fingerprint(&refreshed_seed, &context)
        );
    }

    #[test]
    fn live_convergence_sets_the_websocket_identity_without_mutating_the_payload() {
        let transport = sample_transport();
        let original_body = json!({"model": "gpt-live", "future_live_field": true});
        let mut body = original_body.clone();
        let mut headers = BTreeMap::new();

        assert!(apply_codex_fingerprint_convergence(
            &transport,
            "codex:live",
            Some("client-live-session"),
            &mut headers,
            &mut body,
        ));

        assert_eq!(body, original_body);
        assert_eq!(headers.get("x-session-id"), headers.get("thread-id"));
        assert!(headers.contains_key("x-codex-installation-id"));
        assert!(headers.contains_key("x-codex-window-id"));
    }

    #[test]
    fn disabled_or_out_of_scope_requests_are_unchanged() {
        let mut transport = sample_transport();
        let original_headers = BTreeMap::from([("session-id".to_string(), "client".to_string())]);
        let original_body = json!({"model": "gpt-5.4"});

        transport.provider.config = None;
        let mut headers = original_headers.clone();
        let mut body = original_body.clone();
        assert!(!apply_codex_fingerprint_convergence(
            &transport,
            "openai:responses",
            Some("client"),
            &mut headers,
            &mut body,
        ));
        assert_eq!(headers, original_headers);
        assert_eq!(body, original_body);
        transport.provider.config = Some(json!({
            "codex": {"fingerprint_convergence_enabled": true}
        }));

        for api_format in [
            "openai:responses:compact",
            "openai:chat",
            "openai:search",
            "openai:image",
        ] {
            let mut headers = original_headers.clone();
            let mut body = original_body.clone();
            assert!(!apply_codex_fingerprint_convergence(
                &transport,
                api_format,
                Some("client"),
                &mut headers,
                &mut body,
            ));
            assert_eq!(headers, original_headers);
            assert_eq!(body, original_body);
        }

        let mut compact_v2_headers = original_headers.clone();
        let mut compact_v2_body = json!({
            "model": "gpt-5.4",
            "input": [{"type": "compaction_trigger"}]
        });
        let original_compact_v2_body = compact_v2_body.clone();
        assert!(!apply_codex_fingerprint_convergence(
            &transport,
            "openai:responses",
            Some("client"),
            &mut compact_v2_headers,
            &mut compact_v2_body,
        ));
        assert_eq!(compact_v2_headers, original_headers);
        assert_eq!(compact_v2_body, original_compact_v2_body);
    }

    #[test]
    fn ordinary_codex_auth_channels_apply_convergence() {
        for auth_type in ["api_key", "bearer"] {
            let mut transport = sample_transport();
            transport.key.auth_type = auth_type.to_string();
            let original_headers =
                BTreeMap::from([("x-custom-header".to_string(), "preserve-me".to_string())]);
            let original_body = json!({"model": "gpt-5.4"});
            let mut headers = original_headers.clone();
            let mut body = original_body.clone();

            assert!(apply_codex_fingerprint_convergence(
                &transport,
                "openai:responses",
                Some("client"),
                &mut headers,
                &mut body,
            ));
            assert_ne!(headers, original_headers, "auth_type={auth_type}");
            assert_ne!(body, original_body, "auth_type={auth_type}");
            assert!(headers.contains_key("x-codex-installation-id"));
            assert_eq!(body["client_metadata"]["session_id"], headers["session-id"]);
        }
    }

    #[test]
    fn non_codex_providers_are_unchanged_even_with_codex_convergence_enabled() {
        let context = CodexFingerprintConvergenceContext::new("logical-turn", 1_700_000_000_123)
            .with_original_turn_id("client-turn")
            .with_original_client_session_id("client-session")
            .with_original_prompt_cache_key("client-cache");

        for provider_type in ["openai", "anthropic", "custom"] {
            let mut transport = sample_transport();
            transport.provider.provider_type = provider_type.to_string();
            let original_headers = BTreeMap::from([
                ("session-id".to_string(), "client-session".to_string()),
                (
                    "x-codex-turn-metadata".to_string(),
                    json!({"turn_id": "client-turn"}).to_string(),
                ),
                ("x-custom-header".to_string(), "preserve-me".to_string()),
            ]);
            let original_body = json!({
                "model": "gpt-5.4",
                "prompt_cache_key": "client-cache",
                "client_metadata": {"session_id": "client-session"}
            });
            let mut headers = original_headers.clone();
            let mut body = original_body.clone();

            assert!(!apply_codex_fingerprint_convergence_with_context(
                &transport,
                "openai:responses",
                &context,
                &mut headers,
                &mut body,
            ));
            assert_eq!(headers, original_headers, "provider={provider_type}");
            assert_eq!(body, original_body, "provider={provider_type}");
        }
    }

    #[test]
    fn ordinary_codex_auth_channels_are_stable_and_key_scoped() {
        let context = CodexFingerprintConvergenceContext::new("logical-turn", 1_700_000_000_123)
            .with_original_client_session_id("client-session");
        for auth_type in ["api_key", "bearer"] {
            let mut transport = sample_transport();
            transport.key.auth_type = auth_type.to_string();
            transport.key.decrypted_auth_config = None;

            let mut first_headers = BTreeMap::new();
            let mut first_body = json!({"model": "gpt-5.4"});
            assert!(apply_codex_fingerprint_convergence_with_context(
                &transport,
                "openai:responses",
                &context,
                &mut first_headers,
                &mut first_body,
            ));

            let mut retry_headers = BTreeMap::new();
            let mut retry_body = json!({"model": "gpt-5.4"});
            assert!(apply_codex_fingerprint_convergence_with_context(
                &transport,
                "openai:responses",
                &context,
                &mut retry_headers,
                &mut retry_body,
            ));
            assert_eq!(first_headers, retry_headers, "auth_type={auth_type}");
            assert_eq!(first_body, retry_body, "auth_type={auth_type}");

            transport.key.id = "key-2".to_string();
            let mut other_key_headers = BTreeMap::new();
            let mut other_key_body = json!({"model": "gpt-5.4"});
            assert!(apply_codex_fingerprint_convergence_with_context(
                &transport,
                "openai:responses",
                &context,
                &mut other_key_headers,
                &mut other_key_body,
            ));
            assert_ne!(
                first_headers["x-codex-installation-id"],
                other_key_headers["x-codex-installation-id"],
                "auth_type={auth_type}"
            );
            assert_ne!(
                first_body["client_metadata"], other_key_body["client_metadata"],
                "auth_type={auth_type}"
            );
        }
    }

    #[test]
    fn agent_identity_transport_is_unchanged() {
        let mut transport = sample_transport();
        transport.key.decrypted_auth_config = Some(
            json!({
                "auth_mode": "agentIdentity",
                "agent_identity": {
                    "agent_runtime_id": "runtime-1",
                    "agent_private_key": "private-key"
                }
            })
            .to_string(),
        );
        let original_headers = BTreeMap::from([("session-id".to_string(), "client".to_string())]);
        let original_body = json!({"model": "gpt-5.4"});
        let mut headers = original_headers.clone();
        let mut body = original_body.clone();

        assert!(!apply_codex_fingerprint_convergence(
            &transport,
            "openai:responses",
            Some("client"),
            &mut headers,
            &mut body,
        ));
        assert_eq!(headers, original_headers);
        assert_eq!(body, original_body);

        let context = ProviderOutboundRequestContext::new("logical-turn", 1_700_000_000_123);
        let result = apply_codex_fingerprint_convergence_policy(
            &transport,
            "openai:responses",
            &context,
            &mut headers,
            &mut body,
        );
        assert_eq!(
            result.reason,
            ProviderOutboundRequestPolicyReason::AgentIdentityExcluded
        );
        assert_eq!(headers, original_headers);
        assert_eq!(body, original_body);
    }

    #[test]
    fn policy_result_reports_applied_scopes_without_identity_values() {
        let transport = sample_transport();
        let context = ProviderOutboundRequestContext::new("logical-turn", 1_700_000_000_123);
        let mut headers = BTreeMap::new();
        let mut body = json!({"model": "gpt-5.4"});

        let result = apply_codex_fingerprint_convergence_policy(
            &transport,
            "openai:responses",
            &context,
            &mut headers,
            &mut body,
        );

        assert_eq!(
            result,
            ProviderOutboundRequestPolicyResult {
                policy: ProviderOutboundRequestPolicy::CodexFingerprintConvergence,
                outcome:
                    crate::outbound_request_policy::ProviderOutboundRequestPolicyOutcome::Applied,
                reason: ProviderOutboundRequestPolicyReason::Applied,
                mutation_scope: Some(ProviderOutboundRequestMutationScope::HeadersAndBody),
                identity_scope: Some(ProviderOutboundRequestIdentityScope::Account),
            }
        );
        let serialized = serde_json::to_value(result).expect("serialize policy result");
        let serialized = serialized.as_object().expect("policy result object");
        assert_eq!(serialized.len(), 5);
        assert!(!serialized.contains_key("installation_id"));
        assert!(!serialized.contains_key("session_id"));
        assert!(!serialized.contains_key("turn_id"));
    }

    #[test]
    fn policy_result_distinguishes_codex_skip_reasons_without_mutation() {
        let context = ProviderOutboundRequestContext::new("logical-turn", 1_700_000_000_123);
        let original_headers = BTreeMap::from([("x-custom".to_string(), "preserve".to_string())]);
        let original_body = json!({"model": "gpt-5.4"});

        let cases = [
            (
                "provider_type_mismatch",
                "openai",
                Some(json!({"codex": {"fingerprint_convergence_enabled": true}})),
                "openai:responses",
                original_body.clone(),
                ProviderOutboundRequestPolicyReason::ProviderTypeMismatch,
            ),
            (
                "unsupported_api_format",
                "codex",
                Some(json!({"codex": {"fingerprint_convergence_enabled": true}})),
                "openai:chat",
                original_body.clone(),
                ProviderOutboundRequestPolicyReason::UnsupportedApiFormat,
            ),
            (
                "compact_operation",
                "codex",
                Some(json!({"codex": {"fingerprint_convergence_enabled": true}})),
                "openai:responses",
                json!({"model": "gpt-5.4", "input": [{"type": "compaction_trigger"}]}),
                ProviderOutboundRequestPolicyReason::CompactOperationExcluded,
            ),
            (
                "disabled",
                "codex",
                None,
                "openai:responses",
                original_body.clone(),
                ProviderOutboundRequestPolicyReason::Disabled,
            ),
            (
                "request_body_not_object",
                "codex",
                Some(json!({"codex": {"fingerprint_convergence_enabled": true}})),
                "openai:responses",
                json!(["not-an-object"]),
                ProviderOutboundRequestPolicyReason::RequestBodyNotObject,
            ),
        ];

        for (name, provider_type, config, api_format, body, expected_reason) in cases {
            let mut transport = sample_transport();
            transport.provider.provider_type = provider_type.to_string();
            transport.provider.config = config;
            let mut headers = original_headers.clone();
            let mut request_body = body.clone();

            let result = apply_codex_fingerprint_convergence_policy(
                &transport,
                api_format,
                &context,
                &mut headers,
                &mut request_body,
            );

            assert_eq!(
                result.outcome,
                crate::outbound_request_policy::ProviderOutboundRequestPolicyOutcome::Skipped,
                "case={name}"
            );
            assert_eq!(result.reason, expected_reason, "case={name}");
            assert_eq!(result.mutation_scope, None, "case={name}");
            assert_eq!(result.identity_scope, None, "case={name}");
            assert_eq!(headers, original_headers, "case={name}");
            assert_eq!(request_body, body, "case={name}");
        }
    }
}
