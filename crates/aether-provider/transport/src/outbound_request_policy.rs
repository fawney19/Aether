use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::snapshot::GatewayProviderTransportSnapshot;

/// Stable request-level signals that provider-specific outbound policies may use.
///
/// The context deliberately carries client inputs rather than provider-derived
/// identities. Policies remain responsible for deriving and applying their own
/// wire representation at the terminal transport boundary.
const CONTEXT_HASH_DOMAIN: &[u8] = b"aether-provider-outbound-context-v1";
const CONTEXT_HASH_PREFIX: &str = "aether:provider-context:v1:";
/// Maximum byte length of any value retained in a cross-stage provider context.
///
/// The limit is enforced before a context can reach Live persistence. Values
/// above the limit are represented by a deterministic, field-scoped digest so
/// retries keep the same policy identity without allowing unbounded client
/// input into the registry.
pub const PROVIDER_OUTBOUND_CONTEXT_MAX_VALUE_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderOutboundRequestContext {
    logical_turn_id: String,
    original_turn_id: Option<String>,
    original_client_session_id: Option<String>,
    original_prompt_cache_key: Option<String>,
    turn_started_at_unix_ms: u64,
}

impl ProviderOutboundRequestContext {
    pub fn new(logical_turn_id: impl Into<String>, turn_started_at_unix_ms: u64) -> Self {
        Self {
            logical_turn_id: canonical_required_value(logical_turn_id.into(), "logical_turn_id"),
            original_turn_id: None,
            original_client_session_id: None,
            original_prompt_cache_key: None,
            turn_started_at_unix_ms,
        }
    }

    pub fn with_original_turn_id(mut self, original_turn_id: impl Into<String>) -> Self {
        self.original_turn_id = canonical_optional_value(original_turn_id.into(), "turn_id");
        self
    }

    pub fn with_original_client_session_id(
        mut self,
        original_client_session_id: impl Into<String>,
    ) -> Self {
        self.original_client_session_id =
            canonical_optional_value(original_client_session_id.into(), "client_session_id");
        self
    }

    pub fn with_original_prompt_cache_key(
        mut self,
        original_prompt_cache_key: impl Into<String>,
    ) -> Self {
        self.original_prompt_cache_key =
            canonical_optional_value(original_prompt_cache_key.into(), "prompt_cache_key");
        self
    }

    pub fn logical_turn_id(&self) -> &str {
        self.logical_turn_id.as_str()
    }

    pub fn original_turn_id(&self) -> Option<&str> {
        self.original_turn_id.as_deref()
    }

    pub fn original_client_session_id(&self) -> Option<&str> {
        self.original_client_session_id.as_deref()
    }

    pub fn original_prompt_cache_key(&self) -> Option<&str> {
        self.original_prompt_cache_key.as_deref()
    }

    pub fn turn_started_at_unix_ms(&self) -> u64 {
        self.turn_started_at_unix_ms
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderOutboundRequestPolicy {
    CodexFingerprintConvergence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderOutboundRequestPolicyOutcome {
    Applied,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderOutboundRequestPolicyReason {
    Applied,
    ProviderTypeMismatch,
    AgentIdentityExcluded,
    UnsupportedApiFormat,
    CompactOperationExcluded,
    Disabled,
    RequestBodyNotObject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderOutboundRequestMutationScope {
    Headers,
    Body,
    HeadersAndBody,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderOutboundRequestIdentityScope {
    PersistedFingerprint,
    AccountMember,
    Member,
    Account,
    Key,
}

/// Low-sensitivity report for one selected provider-specific policy.
///
/// This type intentionally contains only categorical values. Derived identity
/// values, client identifiers, and cache keys must never be added to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderOutboundRequestPolicyResult {
    pub policy: ProviderOutboundRequestPolicy,
    pub outcome: ProviderOutboundRequestPolicyOutcome,
    pub reason: ProviderOutboundRequestPolicyReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mutation_scope: Option<ProviderOutboundRequestMutationScope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity_scope: Option<ProviderOutboundRequestIdentityScope>,
}

impl ProviderOutboundRequestPolicyResult {
    pub(crate) fn applied(
        policy: ProviderOutboundRequestPolicy,
        mutation_scope: ProviderOutboundRequestMutationScope,
        identity_scope: ProviderOutboundRequestIdentityScope,
    ) -> Self {
        Self {
            policy,
            outcome: ProviderOutboundRequestPolicyOutcome::Applied,
            reason: ProviderOutboundRequestPolicyReason::Applied,
            mutation_scope: Some(mutation_scope),
            identity_scope: Some(identity_scope),
        }
    }

    pub(crate) fn skipped(
        policy: ProviderOutboundRequestPolicy,
        reason: ProviderOutboundRequestPolicyReason,
    ) -> Self {
        Self {
            policy,
            outcome: ProviderOutboundRequestPolicyOutcome::Skipped,
            reason,
            mutation_scope: None,
            identity_scope: None,
        }
    }

    pub fn was_applied(&self) -> bool {
        self.outcome == ProviderOutboundRequestPolicyOutcome::Applied
    }
}

/// Applies the statically registered outbound policies for the final provider.
///
/// Provider selection has already completed at this boundary. A provider with
/// no registered adapter is a strict no-op and produces no policy result.
pub fn apply_provider_outbound_request_policies(
    transport: &GatewayProviderTransportSnapshot,
    provider_api_format: &str,
    context: &ProviderOutboundRequestContext,
    provider_request_headers: &mut BTreeMap<String, String>,
    provider_request_body: &mut Value,
) -> Vec<ProviderOutboundRequestPolicyResult> {
    if !transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("codex")
    {
        return Vec::new();
    }

    vec![
        crate::codex_fingerprint::apply_codex_fingerprint_convergence_policy(
            transport,
            provider_api_format,
            context,
            provider_request_headers,
            provider_request_body,
        ),
    ]
}

fn canonical_required_value(value: String, field: &str) -> String {
    let value = value.trim();
    // Bound the JSON-encoded representation, not just the source bytes. This
    // preserves ordinary Unicode while preventing quotes, backslashes, or
    // control characters from escaping beyond the aggregate Live record
    // budget.
    if value.len() <= PROVIDER_OUTBOUND_CONTEXT_MAX_VALUE_BYTES
        && serde_json::to_string(value).is_ok_and(|encoded| {
            encoded.len() <= PROVIDER_OUTBOUND_CONTEXT_MAX_VALUE_BYTES.saturating_add(2)
        })
    {
        return value.to_string();
    }
    digest_context_value(field, value)
}

fn canonical_optional_value(value: String, field: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| canonical_required_value(value.to_string(), field))
}

fn digest_context_value(field: &str, value: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(CONTEXT_HASH_DOMAIN);
    digest.update([0]);
    digest.update(field.as_bytes());
    digest.update([0]);
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value.as_bytes());
    format!("{CONTEXT_HASH_PREFIX}{field}:{:x}", digest.finalize())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider,
    };

    fn sample_transport(provider_type: &str) -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "Provider".to_string(),
                provider_type: provider_type.to_string(),
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
                    "codex": {"fingerprint_convergence_enabled": true}
                })),
            },
            endpoint: GatewayProviderTransportEndpoint {
                id: "endpoint-1".to_string(),
                provider_id: "provider-1".to_string(),
                api_format: "openai:responses".to_string(),
                api_family: None,
                endpoint_kind: None,
                is_active: true,
                base_url: "https://example.com".to_string(),
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
                name: "Key".to_string(),
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
                upstream_metadata: None,
                decrypted_api_key: "secret".to_string(),
                decrypted_auth_config: None,
            },
        }
    }

    #[test]
    fn dispatcher_leaves_unregistered_provider_requests_unchanged() {
        let transport = sample_transport("openai");
        let context = ProviderOutboundRequestContext::new("logical-turn", 1_700_000_000_123);
        let original_headers = BTreeMap::from([("x-custom".to_string(), "preserve".to_string())]);
        let original_body = json!({"model": "gpt-5.4", "custom": true});
        let mut headers = original_headers.clone();
        let mut body = original_body.clone();

        let results = apply_provider_outbound_request_policies(
            &transport,
            "openai:responses",
            &context,
            &mut headers,
            &mut body,
        );

        assert!(results.is_empty());
        assert_eq!(headers, original_headers);
        assert_eq!(body, original_body);
    }

    #[test]
    fn result_serialization_is_categorical_and_snake_case() {
        let result = ProviderOutboundRequestPolicyResult::applied(
            ProviderOutboundRequestPolicy::CodexFingerprintConvergence,
            ProviderOutboundRequestMutationScope::HeadersAndBody,
            ProviderOutboundRequestIdentityScope::AccountMember,
        );

        assert_eq!(
            serde_json::to_value(result).expect("serialize policy result"),
            json!({
                "policy": "codex_fingerprint_convergence",
                "outcome": "applied",
                "reason": "applied",
                "mutation_scope": "headers_and_body",
                "identity_scope": "account_member"
            })
        );
    }

    #[test]
    fn context_values_are_bounded_and_deterministic() {
        let oversized = "x".repeat(PROVIDER_OUTBOUND_CONTEXT_MAX_VALUE_BYTES + 1);
        let context = ProviderOutboundRequestContext::new(oversized.clone(), 1)
            .with_original_turn_id(oversized.clone())
            .with_original_client_session_id(oversized.clone())
            .with_original_prompt_cache_key(oversized);
        let same_context = ProviderOutboundRequestContext::new(
            "x".repeat(PROVIDER_OUTBOUND_CONTEXT_MAX_VALUE_BYTES + 1),
            1,
        )
        .with_original_turn_id("x".repeat(PROVIDER_OUTBOUND_CONTEXT_MAX_VALUE_BYTES + 1))
        .with_original_client_session_id("x".repeat(PROVIDER_OUTBOUND_CONTEXT_MAX_VALUE_BYTES + 1))
        .with_original_prompt_cache_key("x".repeat(PROVIDER_OUTBOUND_CONTEXT_MAX_VALUE_BYTES + 1));

        assert_eq!(context, same_context);
        assert!(context.logical_turn_id().len() <= PROVIDER_OUTBOUND_CONTEXT_MAX_VALUE_BYTES);
        assert!(context
            .original_turn_id()
            .is_some_and(|value| value.len() <= PROVIDER_OUTBOUND_CONTEXT_MAX_VALUE_BYTES));
        assert!(context
            .original_client_session_id()
            .is_some_and(|value| value.len() <= PROVIDER_OUTBOUND_CONTEXT_MAX_VALUE_BYTES));
        assert!(context
            .original_prompt_cache_key()
            .is_some_and(|value| value.len() <= PROVIDER_OUTBOUND_CONTEXT_MAX_VALUE_BYTES));
        assert!(context.logical_turn_id().starts_with(CONTEXT_HASH_PREFIX));
        assert_ne!(
            context.logical_turn_id(),
            ProviderOutboundRequestContext::new(
                "y".repeat(PROVIDER_OUTBOUND_CONTEXT_MAX_VALUE_BYTES + 1),
                1,
            )
            .logical_turn_id()
        );

        let unicode = ProviderOutboundRequestContext::new("turn-你好", 1);
        assert_eq!(unicode.logical_turn_id(), "turn-你好");
        let escaped = ProviderOutboundRequestContext::new(
            r#"""#.repeat(PROVIDER_OUTBOUND_CONTEXT_MAX_VALUE_BYTES),
            1,
        );
        assert!(escaped.logical_turn_id().starts_with(CONTEXT_HASH_PREFIX));
    }
}
