//! Short-lived ownership registry for Responses WebSocket continuations.
//!
//! OpenAI response IDs are opaque bearer-like references to provider state. A
//! response created on one physical provider binding must never be resumed on
//! a scheduler-selected replacement. This registry stores only non-secret
//! routing metadata and one-way contract fingerprints. Raw response IDs,
//! downstream credentials and upstream credentials are never persisted.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aether_runtime_state::{RuntimeLockLease, RuntimeState};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::binding::UpstreamBindingIdentity;
use super::request::{ResponsesLiteStaticConfig, MAX_RESPONSES_WEBSOCKET_RESPONSE_ID_BYTES};
use crate::ai_serving::{ResponsesWebSocketBodyNormalization, ResponsesWebSocketPinnedCandidate};
use crate::orchestration::ResponsesWebSocketAdapter;

const CONTINUATION_RECORD_SCHEMA_VERSION: u16 = 1;
const CONTINUATION_KEY_PREFIX: &str = "responses_ws:continuation:v1:";
const CONTINUATION_KEY_DOMAIN: &[u8] = b"aether-responses-websocket-continuation-key-v1";
const CONTINUATION_INDEX_PREFIX: &str = "responses_ws:continuation_index:v1:";
const CONTINUATION_INDEX_DOMAIN: &[u8] = b"aether-responses-websocket-continuation-index-v1";
const CONTINUATION_LOCK_PREFIX: &str = "responses_ws:continuation_lock:v1:";
const CONTINUATION_RECORD_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const CONTINUATION_INDEX_LOCK_TTL: Duration = Duration::from_secs(2);
const CONTINUATION_INDEX_LOCK_ACQUIRE_TIMEOUT: Duration = Duration::from_millis(250);
const CONTINUATION_INDEX_LOCK_INITIAL_RETRY_DELAY: Duration = Duration::from_millis(5);
const CONTINUATION_INDEX_LOCK_MAX_RETRY_DELAY: Duration = Duration::from_millis(50);
const CONTINUATION_INDEX_LOCK_OWNER: &str = "responses_ws_continuation_registry";
const MAX_CONTINUATION_RECORDS_PER_PRINCIPAL: usize = 1_024;
const MAX_SERIALIZED_CONTINUATION_RECORD_BYTES: usize = 16 * 1024;
const MAX_CONTINUATION_PRINCIPAL_BYTES: usize = 256;
const MAX_CONTINUATION_RECORD_ID_BYTES: usize = 256;

#[derive(Debug, thiserror::Error)]
pub(super) enum ResponsesWebSocketContinuationRegistryError {
    #[error("invalid Responses WebSocket continuation identity: {0}")]
    InvalidIdentity(&'static str),
    #[error("invalid Responses WebSocket continuation record: {0}")]
    InvalidRecord(&'static str),
    #[error("Responses WebSocket continuation registry serialization failed")]
    Serialization(#[source] serde_json::Error),
    #[error("Responses WebSocket continuation registry contains a corrupt record")]
    CorruptRecord(#[source] serde_json::Error),
    #[error("Responses WebSocket continuation registry record is too large")]
    RecordTooLarge,
    #[error("Responses WebSocket continuation registry ownership conflict")]
    OwnershipConflict,
    #[error("Responses WebSocket continuation registry capacity lock is busy")]
    CapacityLockBusy,
    #[error("Responses WebSocket continuation registry storage is unavailable")]
    Storage(#[source] aether_runtime_state::DataLayerError),
}

impl ResponsesWebSocketContinuationRegistryError {
    pub(super) const fn kind(&self) -> &'static str {
        match self {
            Self::InvalidIdentity(_) => "invalid_identity",
            Self::InvalidRecord(_) => "invalid_record",
            Self::Serialization(_) => "serialization_failed",
            Self::CorruptRecord(_) => "corrupt_record",
            Self::RecordTooLarge => "record_too_large",
            Self::OwnershipConflict => "ownership_conflict",
            Self::CapacityLockBusy => "capacity_lock_busy",
            Self::Storage(_) => "storage_unavailable",
        }
    }
}

/// Non-secret metadata required to prove ownership of a persisted response.
///
/// The record deliberately contains no raw response ID. Its RuntimeState key
/// is derived from the live authenticated principal plus a SHA-256 digest of
/// the opaque response ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResponsesWebSocketContinuationRecord {
    schema_version: u16,
    pinned_candidate: ResponsesWebSocketPinnedCandidate,
    client_model: String,
    provider_model: String,
    adapter: ResponsesWebSocketAdapter,
    binding_fingerprint: [u8; 32],
    normalization_fingerprint: [u8; 32],
    /// Server-derived replay contract for a provider whose id-less reasoning
    /// state must remain byte-identical. This is trusted only because the
    /// record is created after binding an authenticated provider candidate;
    /// request JSON can never set it.
    #[serde(default)]
    deepseek_opaque_reasoning_replay: bool,
    #[serde(default)]
    xai_encrypted_reasoning_replay: bool,
    /// A prior turn stored PII sentinels whose restore mapping exists only on
    /// the original downstream socket. Such a chain cannot safely resume on a
    /// new socket without leaking sentinels, so lookup succeeds but bootstrap
    /// rejects it before contacting the provider.
    #[serde(default)]
    has_connection_local_redaction: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    responses_lite_static_config: Option<ResponsesLiteStaticConfig>,
}

impl ResponsesWebSocketContinuationRecord {
    pub(super) fn from_binding(
        pinned_candidate: ResponsesWebSocketPinnedCandidate,
        client_model: &str,
        provider_model: &str,
        binding: &UpstreamBindingIdentity,
        normalization: &ResponsesWebSocketBodyNormalization,
        has_connection_local_redaction: bool,
        responses_lite_static_config: Option<ResponsesLiteStaticConfig>,
    ) -> Result<Self, ResponsesWebSocketContinuationRegistryError> {
        let record = Self {
            schema_version: CONTINUATION_RECORD_SCHEMA_VERSION,
            pinned_candidate,
            client_model: client_model.to_string(),
            provider_model: provider_model.to_string(),
            adapter: binding.adapter_kind(),
            binding_fingerprint: binding.continuation_fingerprint(),
            normalization_fingerprint: normalization.continuation_fingerprint(),
            deepseek_opaque_reasoning_replay: matches!(
                normalization.reasoning_replay_policy(),
                crate::ai_serving::OpenAiResponsesReasoningReplayPolicy::DeepSeekOpaque
            ),
            xai_encrypted_reasoning_replay: matches!(
                normalization.reasoning_replay_policy(),
                crate::ai_serving::OpenAiResponsesReasoningReplayPolicy::XaiEncrypted
            ),
            has_connection_local_redaction,
            responses_lite_static_config,
        };
        record.validate()?;
        Ok(record)
    }

    pub(super) fn pinned_candidate(&self) -> &ResponsesWebSocketPinnedCandidate {
        &self.pinned_candidate
    }

    pub(super) fn client_model(&self) -> &str {
        self.client_model.as_str()
    }

    pub(super) fn provider_model(&self) -> &str {
        self.provider_model.as_str()
    }

    pub(super) fn adapter(&self) -> ResponsesWebSocketAdapter {
        self.adapter
    }

    pub(super) fn responses_lite_static_config(&self) -> Option<&ResponsesLiteStaticConfig> {
        self.responses_lite_static_config.as_ref()
    }

    pub(super) fn has_connection_local_redaction(&self) -> bool {
        self.has_connection_local_redaction
    }

    pub(super) fn reasoning_replay_policy(
        &self,
    ) -> crate::ai_serving::OpenAiResponsesReasoningReplayPolicy {
        if self.xai_encrypted_reasoning_replay {
            crate::ai_serving::OpenAiResponsesReasoningReplayPolicy::XaiEncrypted
        } else if self.deepseek_opaque_reasoning_replay {
            crate::ai_serving::OpenAiResponsesReasoningReplayPolicy::DeepSeekOpaque
        } else {
            crate::ai_serving::OpenAiResponsesReasoningReplayPolicy::OpenAiItemIds
        }
    }

    pub(super) fn matches_contract(
        &self,
        binding: &UpstreamBindingIdentity,
        normalization: &ResponsesWebSocketBodyNormalization,
    ) -> bool {
        self.adapter == binding.adapter_kind()
            && self.binding_fingerprint == binding.continuation_fingerprint()
            && self.normalization_fingerprint == normalization.continuation_fingerprint()
    }

    fn validate(&self) -> Result<(), ResponsesWebSocketContinuationRegistryError> {
        if self.schema_version != CONTINUATION_RECORD_SCHEMA_VERSION {
            return Err(ResponsesWebSocketContinuationRegistryError::InvalidRecord(
                "unsupported_schema_version",
            ));
        }
        validate_record_identifier(self.pinned_candidate.provider_id(), "invalid_provider_id")?;
        validate_record_identifier(self.pinned_candidate.endpoint_id(), "invalid_endpoint_id")?;
        validate_record_identifier(self.pinned_candidate.key_id(), "invalid_key_id")?;
        validate_record_identifier(self.client_model.as_str(), "invalid_client_model")?;
        validate_record_identifier(self.provider_model.as_str(), "invalid_provider_model")?;
        Ok(())
    }
}

pub(super) struct ResponsesWebSocketContinuationRegistry<'a> {
    runtime_state: &'a RuntimeState,
    ttl: Duration,
    max_records_per_principal: usize,
}

impl<'a> ResponsesWebSocketContinuationRegistry<'a> {
    pub(super) fn new(runtime_state: &'a RuntimeState) -> Self {
        Self {
            runtime_state,
            ttl: CONTINUATION_RECORD_TTL,
            max_records_per_principal: MAX_CONTINUATION_RECORDS_PER_PRINCIPAL,
        }
    }

    #[cfg(test)]
    fn with_limits(runtime_state: &'a RuntimeState, ttl: Duration, max_records: usize) -> Self {
        Self {
            runtime_state,
            ttl,
            max_records_per_principal: max_records,
        }
    }

    pub(super) async fn register(
        &self,
        user_id: &str,
        api_key_id: &str,
        response_id: &str,
        record: &ResponsesWebSocketContinuationRecord,
    ) -> Result<(), ResponsesWebSocketContinuationRegistryError> {
        let key = continuation_registry_key(user_id, api_key_id, response_id)?;
        let index_key = continuation_registry_index_key(user_id, api_key_id)?;
        let lock_key = continuation_registry_lock_key(user_id, api_key_id)?;
        record.validate()?;
        let serialized = serde_json::to_string(record)
            .map_err(ResponsesWebSocketContinuationRegistryError::Serialization)?;
        if serialized.len() > MAX_SERIALIZED_CONTINUATION_RECORD_BYTES {
            return Err(ResponsesWebSocketContinuationRegistryError::RecordTooLarge);
        }
        let lease = self.acquire_capacity_lock(&lock_key).await?;
        let result = self
            .register_under_capacity_lock(&key, &index_key, serialized, record)
            .await;
        let release_result = self.runtime_state.lock_release(&lease).await;
        match (result, release_result) {
            (Err(error), _) => Err(error),
            (Ok(()), Ok(_)) => Ok(()),
            (Ok(()), Err(error)) => {
                Err(ResponsesWebSocketContinuationRegistryError::Storage(error))
            }
        }
    }

    async fn acquire_capacity_lock(
        &self,
        lock_key: &str,
    ) -> Result<RuntimeLockLease, ResponsesWebSocketContinuationRegistryError> {
        let deadline = tokio::time::Instant::now() + CONTINUATION_INDEX_LOCK_ACQUIRE_TIMEOUT;
        let mut retry_delay = CONTINUATION_INDEX_LOCK_INITIAL_RETRY_DELAY;
        loop {
            if let Some(lease) = self
                .runtime_state
                .lock_try_acquire(
                    lock_key,
                    CONTINUATION_INDEX_LOCK_OWNER,
                    CONTINUATION_INDEX_LOCK_TTL,
                )
                .await
                .map_err(ResponsesWebSocketContinuationRegistryError::Storage)?
            {
                return Ok(lease);
            }

            let now = tokio::time::Instant::now();
            if now >= deadline {
                return Err(ResponsesWebSocketContinuationRegistryError::CapacityLockBusy);
            }
            tokio::time::sleep(retry_delay.min(deadline.saturating_duration_since(now))).await;
            retry_delay = retry_delay
                .saturating_mul(2)
                .min(CONTINUATION_INDEX_LOCK_MAX_RETRY_DELAY);
        }
    }

    async fn register_under_capacity_lock(
        &self,
        key: &str,
        index_key: &str,
        serialized: String,
        record: &ResponsesWebSocketContinuationRecord,
    ) -> Result<(), ResponsesWebSocketContinuationRegistryError> {
        if let Some(existing) = self
            .runtime_state
            .kv_get(key)
            .await
            .map_err(ResponsesWebSocketContinuationRegistryError::Storage)?
        {
            let existing = serde_json::from_str::<ResponsesWebSocketContinuationRecord>(&existing)
                .map_err(ResponsesWebSocketContinuationRegistryError::CorruptRecord)?;
            if existing != *record {
                return Err(ResponsesWebSocketContinuationRegistryError::OwnershipConflict);
            }
        }
        self.runtime_state
            .kv_set(key, serialized, Some(self.ttl))
            .await
            .map_err(ResponsesWebSocketContinuationRegistryError::Storage)?;
        let score = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as f64;
        if let Err(error) = self.runtime_state.score_set(index_key, key, score).await {
            let _ = self.runtime_state.kv_delete(key).await;
            return Err(ResponsesWebSocketContinuationRegistryError::Storage(error));
        }
        if let Err(error) = self.runtime_state.key_expire(index_key, self.ttl).await {
            let _ = self.runtime_state.score_remove(index_key, key).await;
            let _ = self.runtime_state.kv_delete(key).await;
            return Err(ResponsesWebSocketContinuationRegistryError::Storage(error));
        }
        let members = self
            .runtime_state
            .score_range_by_min(index_key, f64::NEG_INFINITY)
            .await
            .map_err(ResponsesWebSocketContinuationRegistryError::Storage)?;
        let overflow = members.len().saturating_sub(self.max_records_per_principal);
        for oldest_key in members.into_iter().take(overflow) {
            self.runtime_state
                .kv_delete(&oldest_key)
                .await
                .map_err(ResponsesWebSocketContinuationRegistryError::Storage)?;
            self.runtime_state
                .score_remove(index_key, &oldest_key)
                .await
                .map_err(ResponsesWebSocketContinuationRegistryError::Storage)?;
        }
        Ok(())
    }

    pub(super) async fn lookup(
        &self,
        user_id: &str,
        api_key_id: &str,
        response_id: &str,
    ) -> Result<
        Option<ResponsesWebSocketContinuationRecord>,
        ResponsesWebSocketContinuationRegistryError,
    > {
        let key = continuation_registry_key(user_id, api_key_id, response_id)?;
        let Some(serialized) = self
            .runtime_state
            .kv_get(&key)
            .await
            .map_err(ResponsesWebSocketContinuationRegistryError::Storage)?
        else {
            return Ok(None);
        };
        let record = serde_json::from_str::<ResponsesWebSocketContinuationRecord>(&serialized)
            .map_err(ResponsesWebSocketContinuationRegistryError::CorruptRecord)?;
        record.validate()?;
        Ok(Some(record))
    }
}

fn validate_record_identifier(
    value: &str,
    error: &'static str,
) -> Result<(), ResponsesWebSocketContinuationRegistryError> {
    if value.trim().is_empty() || value.len() > MAX_CONTINUATION_RECORD_ID_BYTES {
        return Err(ResponsesWebSocketContinuationRegistryError::InvalidRecord(
            error,
        ));
    }
    Ok(())
}

fn validate_key_component(
    value: &str,
    max_bytes: usize,
    error: &'static str,
) -> Result<(), ResponsesWebSocketContinuationRegistryError> {
    if value.is_empty() || value.len() > max_bytes {
        return Err(ResponsesWebSocketContinuationRegistryError::InvalidIdentity(error));
    }
    Ok(())
}

fn continuation_registry_key(
    user_id: &str,
    api_key_id: &str,
    response_id: &str,
) -> Result<String, ResponsesWebSocketContinuationRegistryError> {
    validate_key_component(user_id, MAX_CONTINUATION_PRINCIPAL_BYTES, "invalid_user_id")?;
    validate_key_component(
        api_key_id,
        MAX_CONTINUATION_PRINCIPAL_BYTES,
        "invalid_api_key_id",
    )?;
    validate_key_component(
        response_id,
        MAX_RESPONSES_WEBSOCKET_RESPONSE_ID_BYTES,
        "invalid_response_id",
    )?;

    let digest = digest_key_components(
        CONTINUATION_KEY_DOMAIN,
        [
            user_id.as_bytes(),
            api_key_id.as_bytes(),
            response_id.as_bytes(),
        ],
    );
    Ok(format!("{CONTINUATION_KEY_PREFIX}{digest}"))
}

fn continuation_registry_index_key(
    user_id: &str,
    api_key_id: &str,
) -> Result<String, ResponsesWebSocketContinuationRegistryError> {
    validate_key_component(user_id, MAX_CONTINUATION_PRINCIPAL_BYTES, "invalid_user_id")?;
    validate_key_component(
        api_key_id,
        MAX_CONTINUATION_PRINCIPAL_BYTES,
        "invalid_api_key_id",
    )?;
    let digest = digest_key_components(
        CONTINUATION_INDEX_DOMAIN,
        [user_id.as_bytes(), api_key_id.as_bytes()],
    );
    Ok(format!("{CONTINUATION_INDEX_PREFIX}{digest}"))
}

fn continuation_registry_lock_key(
    user_id: &str,
    api_key_id: &str,
) -> Result<String, ResponsesWebSocketContinuationRegistryError> {
    let index = continuation_registry_index_key(user_id, api_key_id)?;
    Ok(format!(
        "{CONTINUATION_LOCK_PREFIX}{}",
        index
            .strip_prefix(CONTINUATION_INDEX_PREFIX)
            .unwrap_or(index.as_str())
    ))
}

fn digest_key_components<const N: usize>(domain: &[u8], components: [&[u8]; N]) -> String {
    let mut digest = Sha256::new();
    digest.update(domain);
    for component in components {
        digest.update((component.len() as u64).to_be_bytes());
        digest.update(component);
    }
    format!("{:x}", digest.finalize())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use aether_runtime_state::{MemoryRuntimeStateConfig, RuntimeState};
    use serde_json::json;

    use super::*;

    const USER_ID: &str = "user-live-0198";
    const API_KEY_ID: &str = "api-key-live-0198";
    const RESPONSE_ID: &str = "resp_opaque_super_secret_reference";

    fn runtime_state() -> RuntimeState {
        RuntimeState::memory(MemoryRuntimeStateConfig::default())
    }

    fn record() -> ResponsesWebSocketContinuationRecord {
        ResponsesWebSocketContinuationRecord {
            schema_version: CONTINUATION_RECORD_SCHEMA_VERSION,
            pinned_candidate: ResponsesWebSocketPinnedCandidate::new(
                "provider-1",
                "endpoint-1",
                "key-1",
            )
            .expect("candidate"),
            client_model: "public-model".to_string(),
            provider_model: "provider-model".to_string(),
            adapter: ResponsesWebSocketAdapter::Codex,
            binding_fingerprint: [7; 32],
            normalization_fingerprint: [9; 32],
            deepseek_opaque_reasoning_replay: false,
            xai_encrypted_reasoning_replay: false,
            has_connection_local_redaction: false,
            responses_lite_static_config: Some(ResponsesLiteStaticConfig::from_response_create(
                &json!({
                    "tools": [{"type": "function", "name": "lookup"}],
                    "instructions": "Do not persist this plaintext"
                }),
            )),
        }
    }

    #[tokio::test]
    async fn same_principal_gets_the_exact_pinned_candidate_and_other_principals_miss() {
        let runtime = runtime_state();
        let registry = ResponsesWebSocketContinuationRegistry::new(&runtime);
        let expected = record();
        registry
            .register(USER_ID, API_KEY_ID, RESPONSE_ID, &expected)
            .await
            .expect("register");

        let found = registry
            .lookup(USER_ID, API_KEY_ID, RESPONSE_ID)
            .await
            .expect("lookup")
            .expect("same authenticated principal must find its record");
        assert_eq!(found, expected);
        assert_eq!(found.pinned_candidate(), expected.pinned_candidate());
        for (user_id, api_key_id, response_id) in [
            ("other-user", API_KEY_ID, RESPONSE_ID),
            (USER_ID, "other-api-key", RESPONSE_ID),
            (USER_ID, API_KEY_ID, "resp_other"),
        ] {
            assert_eq!(
                registry
                    .lookup(user_id, api_key_id, response_id)
                    .await
                    .expect("isolated lookup"),
                None
            );
        }
    }

    #[tokio::test]
    async fn corrupt_or_unsupported_records_fail_closed() {
        let runtime = runtime_state();
        let registry = ResponsesWebSocketContinuationRegistry::new(&runtime);
        let key = continuation_registry_key(USER_ID, API_KEY_ID, RESPONSE_ID).expect("key");
        runtime
            .kv_set(&key, "not-json", Some(Duration::from_secs(60)))
            .await
            .expect("seed corrupt record");
        assert!(matches!(
            registry.lookup(USER_ID, API_KEY_ID, RESPONSE_ID).await,
            Err(ResponsesWebSocketContinuationRegistryError::CorruptRecord(
                _
            ))
        ));

        let mut unsupported = serde_json::to_value(record()).expect("serialize record");
        unsupported["schema_version"] = json!(CONTINUATION_RECORD_SCHEMA_VERSION + 1);
        runtime
            .kv_set(&key, unsupported.to_string(), Some(Duration::from_secs(60)))
            .await
            .expect("seed unsupported record");
        assert!(matches!(
            registry.lookup(USER_ID, API_KEY_ID, RESPONSE_ID).await,
            Err(ResponsesWebSocketContinuationRegistryError::InvalidRecord(
                "unsupported_schema_version"
            ))
        ));
    }

    #[tokio::test]
    async fn expired_records_are_not_returned() {
        let runtime = runtime_state();
        let registry = ResponsesWebSocketContinuationRegistry::with_limits(
            &runtime,
            Duration::from_millis(5),
            MAX_CONTINUATION_RECORDS_PER_PRINCIPAL,
        );
        registry
            .register(USER_ID, API_KEY_ID, RESPONSE_ID, &record())
            .await
            .expect("register");
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(
            registry
                .lookup(USER_ID, API_KEY_ID, RESPONSE_ID)
                .await
                .expect("lookup"),
            None
        );
    }

    #[tokio::test]
    async fn registry_evicts_the_oldest_record_per_authenticated_principal() {
        let runtime = runtime_state();
        let registry = ResponsesWebSocketContinuationRegistry::with_limits(
            &runtime,
            Duration::from_secs(60),
            2,
        );
        for response_id in ["resp_oldest", "resp_middle", "resp_newest"] {
            registry
                .register(USER_ID, API_KEY_ID, response_id, &record())
                .await
                .expect("register");
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        assert_eq!(
            registry
                .lookup(USER_ID, API_KEY_ID, "resp_oldest")
                .await
                .expect("lookup"),
            None
        );
        for response_id in ["resp_middle", "resp_newest"] {
            assert_eq!(
                registry
                    .lookup(USER_ID, API_KEY_ID, response_id)
                    .await
                    .expect("lookup"),
                Some(record())
            );
        }
        let index_key = continuation_registry_index_key(USER_ID, API_KEY_ID).expect("index key");
        assert_eq!(runtime.score_len(&index_key).await.expect("index len"), 2);
    }

    #[tokio::test]
    async fn registry_retries_a_briefly_contended_principal_capacity_lock() {
        let runtime = runtime_state();
        let registry = ResponsesWebSocketContinuationRegistry::new(&runtime);
        let lock_key = continuation_registry_lock_key(USER_ID, API_KEY_ID).expect("lock key");
        let held = runtime
            .lock_try_acquire(
                &lock_key,
                "continuation-registry-contention-test",
                CONTINUATION_INDEX_LOCK_TTL,
            )
            .await
            .expect("acquire test lock")
            .expect("test lock should be uncontended");

        let release_held_lock = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            assert!(runtime
                .lock_release(&held)
                .await
                .expect("release test lock"));
        };
        let expected = record();
        let (registration, ()) = tokio::join!(
            registry.register(USER_ID, API_KEY_ID, RESPONSE_ID, &expected),
            release_held_lock,
        );

        registration.expect("registration should retry until the short contention clears");
        assert!(registry
            .lookup(USER_ID, API_KEY_ID, RESPONSE_ID)
            .await
            .expect("lookup")
            .is_some());
    }

    #[tokio::test]
    async fn same_response_id_cannot_be_rebound_to_a_different_owner_record() {
        let runtime = runtime_state();
        let registry = ResponsesWebSocketContinuationRegistry::new(&runtime);
        let original = record();
        registry
            .register(USER_ID, API_KEY_ID, RESPONSE_ID, &original)
            .await
            .expect("register");
        let mut replacement = original.clone();
        replacement.provider_model = "different-provider-model".to_string();
        assert!(matches!(
            registry
                .register(USER_ID, API_KEY_ID, RESPONSE_ID, &replacement)
                .await,
            Err(ResponsesWebSocketContinuationRegistryError::OwnershipConflict)
        ));
        assert_eq!(
            registry
                .lookup(USER_ID, API_KEY_ID, RESPONSE_ID)
                .await
                .expect("lookup"),
            Some(original)
        );
    }

    #[test]
    fn registry_key_is_length_delimited_hashed_and_contains_no_plaintext() {
        let key = continuation_registry_key(USER_ID, API_KEY_ID, RESPONSE_ID).expect("key");
        assert!(key.starts_with(CONTINUATION_KEY_PREFIX));
        assert_eq!(key.len(), CONTINUATION_KEY_PREFIX.len() + 64);
        for secret in [USER_ID, API_KEY_ID, RESPONSE_ID, "super_secret_reference"] {
            assert!(!key.contains(secret));
        }
        let index_key = continuation_registry_index_key(USER_ID, API_KEY_ID).expect("index key");
        for secret in [USER_ID, API_KEY_ID] {
            assert!(!index_key.contains(secret));
        }
        assert_ne!(
            continuation_registry_key("ab", "c", "d").expect("key"),
            continuation_registry_key("a", "bc", "d").expect("key")
        );
    }

    #[test]
    fn invalid_or_oversized_identity_never_produces_a_cache_key() {
        for (user_id, api_key_id, response_id) in [
            ("", API_KEY_ID, RESPONSE_ID),
            (USER_ID, "", RESPONSE_ID),
            (USER_ID, API_KEY_ID, ""),
        ] {
            assert!(continuation_registry_key(user_id, api_key_id, response_id).is_err());
        }
        let oversized = "x".repeat(MAX_RESPONSES_WEBSOCKET_RESPONSE_ID_BYTES + 1);
        assert!(continuation_registry_key(USER_ID, API_KEY_ID, &oversized).is_err());
    }

    #[test]
    fn serialized_record_contains_only_digests_for_static_and_binding_state() {
        let serialized = serde_json::to_string(&record()).expect("serialize");
        for plaintext in [
            RESPONSE_ID,
            "Do not persist this plaintext",
            "lookup",
            "upstream-oauth-token",
        ] {
            assert!(!serialized.contains(plaintext));
        }
        let decoded: ResponsesWebSocketContinuationRecord =
            serde_json::from_str(&serialized).expect("deserialize");
        assert_eq!(decoded, record());
    }

    #[test]
    fn serialized_record_preserves_xai_replay_policy_and_reads_legacy_records() {
        let mut expected = record();
        expected.xai_encrypted_reasoning_replay = true;
        let mut serialized = serde_json::to_value(&expected).unwrap();
        let decoded: ResponsesWebSocketContinuationRecord =
            serde_json::from_value(serialized.clone()).unwrap();
        assert_eq!(
            decoded.reasoning_replay_policy(),
            crate::ai_serving::OpenAiResponsesReasoningReplayPolicy::XaiEncrypted
        );
        serialized
            .as_object_mut()
            .unwrap()
            .remove("xai_encrypted_reasoning_replay");
        let legacy: ResponsesWebSocketContinuationRecord =
            serde_json::from_value(serialized).unwrap();
        assert_eq!(
            legacy.reasoning_replay_policy(),
            crate::ai_serving::OpenAiResponsesReasoningReplayPolicy::OpenAiItemIds
        );
    }

    #[test]
    fn serialized_record_preserves_only_the_server_derived_reasoning_replay_policy_bit() {
        let mut expected = record();
        expected.deepseek_opaque_reasoning_replay = true;

        let serialized = serde_json::to_string(&expected).expect("serialize");
        let decoded: ResponsesWebSocketContinuationRecord =
            serde_json::from_str(&serialized).expect("deserialize");
        assert_eq!(
            decoded.reasoning_replay_policy(),
            crate::ai_serving::OpenAiResponsesReasoningReplayPolicy::DeepSeekOpaque
        );

        let mut legacy = serde_json::to_value(&expected).expect("serialize legacy fixture");
        legacy
            .as_object_mut()
            .expect("record is an object")
            .remove("deepseek_opaque_reasoning_replay");
        let legacy: ResponsesWebSocketContinuationRecord =
            serde_json::from_value(legacy).expect("legacy record should remain readable");
        assert_eq!(
            legacy.reasoning_replay_policy(),
            crate::ai_serving::OpenAiResponsesReasoningReplayPolicy::OpenAiItemIds,
            "old records must fail closed instead of inferring trust from request shape"
        );
    }

    #[test]
    fn record_derives_deepseek_replay_policy_from_the_bound_normalization() {
        let decision: crate::ai_serving::AiExecutionDecision = serde_json::from_value(json!({
            "action": "local",
            "provider_id": "provider-1",
            "endpoint_id": "endpoint-1",
            "key_id": "key-1",
            "upstream_url": "https://api.deepseek.com/v1/responses",
            "provider_request_headers": {}
        }))
        .expect("minimal decision");
        let adapter = super::super::adapter::resolve_responses_websocket_adapter(
            ResponsesWebSocketAdapter::Standard,
        );
        let binding =
            UpstreamBindingIdentity::from_decision(adapter, &decision).expect("binding identity");
        let normalization = ResponsesWebSocketBodyNormalization::for_tests("deepseek-reasoner")
            .with_reasoning_replay_policy_for_tests(
                crate::ai_serving::OpenAiResponsesReasoningReplayPolicy::DeepSeekOpaque,
            );
        let record = ResponsesWebSocketContinuationRecord::from_binding(
            ResponsesWebSocketPinnedCandidate::new("provider-1", "endpoint-1", "key-1")
                .expect("pinned candidate"),
            "public-model",
            "deepseek-reasoner",
            &binding,
            &normalization,
            false,
            None,
        )
        .expect("continuation record");

        assert_eq!(
            record.reasoning_replay_policy(),
            crate::ai_serving::OpenAiResponsesReasoningReplayPolicy::DeepSeekOpaque
        );
    }
}
