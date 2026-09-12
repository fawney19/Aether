use async_trait::async_trait;

const REDACTED_DEBUG_VALUE: &str = "[REDACTED]";

fn redacted_debug_option<T>(value: &Option<T>) -> Option<&'static str> {
    value.as_ref().map(|_| REDACTED_DEBUG_VALUE)
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProviderCatalogUpstreamMetadataNamespaceUpdate {
    pub namespace: String,
    pub value: serde_json::Value,
}

impl std::fmt::Debug for ProviderCatalogUpstreamMetadataNamespaceUpdate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderCatalogUpstreamMetadataNamespaceUpdate")
            .field("namespace", &self.namespace)
            .field("value", &REDACTED_DEBUG_VALUE)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProviderCatalogKeyAdaptiveState {
    pub learned_rpm_limit: Option<u32>,
    pub concurrent_429_count: Option<u32>,
    pub rpm_429_count: Option<u32>,
    pub last_429_at_unix_secs: Option<u64>,
    pub last_429_type: Option<String>,
    pub adjustment_history: Option<serde_json::Value>,
    pub utilization_samples: Option<serde_json::Value>,
    pub last_probe_increase_at_unix_secs: Option<u64>,
    pub last_rpm_peak: Option<u32>,
}

impl ProviderCatalogKeyAdaptiveState {
    pub fn canonicalized(&self) -> Self {
        let mut state = self.clone();
        state.concurrent_429_count = Some(state.concurrent_429_count.unwrap_or(0));
        state.rpm_429_count = Some(state.rpm_429_count.unwrap_or(0));
        state
    }
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProviderCatalogKeyAdaptiveStateUpdate {
    pub key_id: String,
    /// Optional auth_config fence for request-owned adaptive feedback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_encrypted_auth_config: Option<String>,
    pub expected: ProviderCatalogKeyAdaptiveState,
    pub next: ProviderCatalogKeyAdaptiveState,
    /// Top-level status fields owned by adaptive rate-limit learning.
    pub status_snapshot_patch: serde_json::Value,
    pub updated_at_unix_secs: Option<u64>,
}

impl std::fmt::Debug for ProviderCatalogKeyAdaptiveStateUpdate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderCatalogKeyAdaptiveStateUpdate")
            .field("key_id", &self.key_id)
            .field(
                "expected_encrypted_auth_config",
                &redacted_debug_option(&self.expected_encrypted_auth_config),
            )
            .field("expected", &self.expected)
            .field("next", &self.next)
            .field("status_snapshot_patch", &REDACTED_DEBUG_VALUE)
            .field("updated_at_unix_secs", &self.updated_at_unix_secs)
            .finish()
    }
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProviderCatalogKeyRuntimeMetadataUpdate {
    pub key_id: String,
    pub namespace: String,
    /// Value observed for `namespace` immediately before calculating the update.
    ///
    /// `None` means that the namespace was absent.  The repository must compare
    /// this value atomically with the stored namespace and return `false` when a
    /// concurrent writer changed it.  This makes read/modify/write metadata
    /// producers safe across gateway instances without replacing the whole
    /// `upstream_metadata` document.
    pub expected_upstream_metadata_value: Option<serde_json::Value>,
    pub upstream_metadata_value: serde_json::Value,
    /// Top-level status fields owned by the metadata producer, normally `quota`.
    pub status_snapshot_patch: serde_json::Value,
    pub updated_at_unix_secs: Option<u64>,
}

impl std::fmt::Debug for ProviderCatalogKeyRuntimeMetadataUpdate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderCatalogKeyRuntimeMetadataUpdate")
            .field("key_id", &self.key_id)
            .field("namespace", &self.namespace)
            .field(
                "expected_upstream_metadata_value",
                &redacted_debug_option(&self.expected_upstream_metadata_value),
            )
            .field("upstream_metadata_value", &REDACTED_DEBUG_VALUE)
            .field("status_snapshot_patch", &REDACTED_DEBUG_VALUE)
            .field("updated_at_unix_secs", &self.updated_at_unix_secs)
            .finish()
    }
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProviderCatalogKeyStatusSnapshotUpdate {
    pub key_id: String,
    /// Top-level status fields owned by the caller.
    pub status_snapshot_patch: serde_json::Value,
    pub updated_at_unix_secs: Option<u64>,
}

impl std::fmt::Debug for ProviderCatalogKeyStatusSnapshotUpdate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderCatalogKeyStatusSnapshotUpdate")
            .field("key_id", &self.key_id)
            .field("status_snapshot_patch", &REDACTED_DEBUG_VALUE)
            .field("updated_at_unix_secs", &self.updated_at_unix_secs)
            .finish()
    }
}

/// Credential context observed before an OAuth refresh started. Repositories
/// compare every field atomically with the runtime-state update so an
/// administrator replacement cannot be overwritten by an older refresh.
#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProviderCatalogKeyOAuthCredentialFence {
    /// Exact nullable ciphertext stored in `provider_api_keys.api_key`.
    pub encrypted_api_key: Option<String>,
    pub auth_type: String,
    pub provider_id: String,
    pub provider_type: String,
}

impl std::fmt::Debug for ProviderCatalogKeyOAuthCredentialFence {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderCatalogKeyOAuthCredentialFence")
            .field(
                "encrypted_api_key",
                &redacted_debug_option(&self.encrypted_api_key),
            )
            .field("auth_type", &self.auth_type)
            .field("provider_id", &self.provider_id)
            .field("provider_type", &self.provider_type)
            .finish()
    }
}

/// Administrator-owned key replacement fenced by the exact credential state
/// observed while the edit was prepared. This prevents an older admin request
/// from restoring credentials that a concurrent request already replaced.
#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProviderCatalogKeyAdminCasUpdate {
    pub expected_encrypted_auth_config: Option<String>,
    pub expected_credential: ProviderCatalogKeyOAuthCredentialFence,
    /// Full requested key. Repositories merge its administrator-owned fields
    /// while preserving the currently stored runtime-owned fields.
    pub key: StoredProviderCatalogKey,
    /// Optional replacement for the complete Codex metadata namespace. When
    /// present it must be an object containing only a non-empty string
    /// `credential_generation`; repositories also clear the quota snapshot in
    /// the same atomic write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_rotation: Option<serde_json::Value>,
    /// Clear OAuth invalid markers in the same credential replacement write.
    /// This must be true whenever new credential material supersedes the old
    /// credential, so a post-CAS unfenced cleanup is never required.
    #[serde(default)]
    pub reset_oauth_runtime: bool,
}

impl std::fmt::Debug for ProviderCatalogKeyAdminCasUpdate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderCatalogKeyAdminCasUpdate")
            .field(
                "expected_encrypted_auth_config",
                &redacted_debug_option(&self.expected_encrypted_auth_config),
            )
            .field("expected_credential", &self.expected_credential)
            .field("key", &self.key)
            .field(
                "codex_rotation",
                &redacted_debug_option(&self.codex_rotation),
            )
            .field("reset_oauth_runtime", &self.reset_oauth_runtime)
            .finish()
    }
}

/// Atomic key deletion fenced by the exact OAuth credential generation that
/// produced the terminal failure and, when supplied, one metadata namespace.
#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProviderCatalogKeyOAuthCredentialCasDelete {
    pub key_id: String,
    pub expected_encrypted_auth_config: Option<String>,
    pub expected_credential: ProviderCatalogKeyOAuthCredentialFence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_upstream_metadata_namespace:
        Option<ProviderCatalogUpstreamMetadataNamespaceExpectation>,
}

impl std::fmt::Debug for ProviderCatalogKeyOAuthCredentialCasDelete {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderCatalogKeyOAuthCredentialCasDelete")
            .field("key_id", &self.key_id)
            .field(
                "expected_encrypted_auth_config",
                &redacted_debug_option(&self.expected_encrypted_auth_config),
            )
            .field("expected_credential", &self.expected_credential)
            .field(
                "expected_upstream_metadata_namespace",
                &self.expected_upstream_metadata_namespace,
            )
            .finish()
    }
}

/// Optional single-namespace metadata fence for an OAuth runtime CAS.
///
/// The outer option on the owning update controls whether the namespace is
/// compared. Within an expectation, `None` requires the namespace to be absent.
#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProviderCatalogUpstreamMetadataNamespaceExpectation {
    pub namespace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_value: Option<serde_json::Value>,
}

impl std::fmt::Debug for ProviderCatalogUpstreamMetadataNamespaceExpectation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderCatalogUpstreamMetadataNamespaceExpectation")
            .field("namespace", &self.namespace)
            .field(
                "expected_value",
                &redacted_debug_option(&self.expected_value),
            )
            .finish()
    }
}

/// Agent/runtime-owned OAuth state update fenced by the exact encrypted
/// auth_config and, when supplied, credential context observed before the
/// refresh started. Repositories must update only these fields and return
/// `false` when an expected value changed.
#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProviderCatalogKeyOAuthRuntimeStateCasUpdate {
    pub key_id: String,
    pub expected_encrypted_auth_config: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_credential: Option<ProviderCatalogKeyOAuthCredentialFence>,
    /// Optional metadata namespace value compared atomically with the OAuth
    /// credential fence. `expected_value: None` means the namespace must be absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_upstream_metadata_namespace:
        Option<ProviderCatalogUpstreamMetadataNamespaceExpectation>,
    pub encrypted_auth_config: String,
    /// Optional access-token ciphertext replacement owned by refresh success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted_api_key_update: Option<String>,
    /// `None` preserves expiry; `Some(None)` clears it; `Some(Some(_))` replaces it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_unix_secs_update: Option<Option<u64>>,
    pub oauth_invalid_at_unix_secs: Option<u64>,
    pub oauth_invalid_reason: Option<String>,
    /// Top-level runtime metadata namespaces to merge in the same fenced write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_metadata_patch: Option<serde_json::Value>,
    /// Optional top-level runtime metadata namespace to remove in the same
    /// fenced write. It must not also appear in `upstream_metadata_patch`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_metadata_namespace_to_remove: Option<String>,
    pub status_snapshot_patch: serde_json::Value,
    #[serde(default)]
    pub reset_error_count: bool,
    pub updated_at_unix_secs: Option<u64>,
}

impl std::fmt::Debug for ProviderCatalogKeyOAuthRuntimeStateCasUpdate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderCatalogKeyOAuthRuntimeStateCasUpdate")
            .field("key_id", &self.key_id)
            .field(
                "expected_encrypted_auth_config",
                &redacted_debug_option(&self.expected_encrypted_auth_config),
            )
            .field("expected_credential", &self.expected_credential)
            .field(
                "expected_upstream_metadata_namespace",
                &self.expected_upstream_metadata_namespace,
            )
            .field("encrypted_auth_config", &REDACTED_DEBUG_VALUE)
            .field(
                "encrypted_api_key_update",
                &redacted_debug_option(&self.encrypted_api_key_update),
            )
            .field(
                "expires_at_unix_secs_update",
                &self.expires_at_unix_secs_update,
            )
            .field(
                "oauth_invalid_at_unix_secs",
                &self.oauth_invalid_at_unix_secs,
            )
            .field(
                "oauth_invalid_reason",
                &redacted_debug_option(&self.oauth_invalid_reason),
            )
            .field(
                "upstream_metadata_patch",
                &redacted_debug_option(&self.upstream_metadata_patch),
            )
            .field(
                "upstream_metadata_namespace_to_remove",
                &self.upstream_metadata_namespace_to_remove,
            )
            .field("status_snapshot_patch", &REDACTED_DEBUG_VALUE)
            .field("reset_error_count", &self.reset_error_count)
            .field("updated_at_unix_secs", &self.updated_at_unix_secs)
            .finish()
    }
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProviderCatalogKeyHealthStateUpdate {
    pub key_id: String,
    /// Optional auth_config fence for lifecycle-owned health recovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_encrypted_auth_config: Option<String>,
    pub expected_health_by_format: Option<serde_json::Value>,
    pub expected_circuit_breaker_by_format: Option<serde_json::Value>,
    pub health_by_format: Option<serde_json::Value>,
    pub circuit_breaker_by_format: Option<serde_json::Value>,
}

impl std::fmt::Debug for ProviderCatalogKeyHealthStateUpdate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderCatalogKeyHealthStateUpdate")
            .field("key_id", &self.key_id)
            .field(
                "expected_encrypted_auth_config",
                &redacted_debug_option(&self.expected_encrypted_auth_config),
            )
            .field("expected_health_by_format", &self.expected_health_by_format)
            .field(
                "expected_circuit_breaker_by_format",
                &self.expected_circuit_breaker_by_format,
            )
            .field("health_by_format", &self.health_by_format)
            .field("circuit_breaker_by_format", &self.circuit_breaker_by_format)
            .finish()
    }
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProviderCatalogProviderConfigCasUpdate {
    pub provider_id: String,
    pub expected_config: Option<serde_json::Value>,
    pub config: Option<serde_json::Value>,
}

impl std::fmt::Debug for ProviderCatalogProviderConfigCasUpdate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderCatalogProviderConfigCasUpdate")
            .field("provider_id", &self.provider_id)
            .field(
                "expected_config",
                &redacted_debug_option(&self.expected_config),
            )
            .field("config", &redacted_debug_option(&self.config))
            .finish()
    }
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProviderCatalogProxyCasUpdate {
    pub record_id: String,
    pub expected_proxy: Option<serde_json::Value>,
    pub proxy: Option<serde_json::Value>,
}

impl std::fmt::Debug for ProviderCatalogProxyCasUpdate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderCatalogProxyCasUpdate")
            .field("record_id", &self.record_id)
            .field(
                "expected_proxy",
                &redacted_debug_option(&self.expected_proxy),
            )
            .field("proxy", &redacted_debug_option(&self.proxy))
            .finish()
    }
}

/// Secret-only migration fenced by the complete catalog-key credential
/// identity. The provider fence prevents a legacy credential from being
/// re-encrypted for an obsolete provider after a concurrent key move.
#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProviderCatalogKeyCredentialsCasUpdate {
    pub key_id: String,
    pub expected_provider_id: String,
    pub expected_encrypted_api_key: Option<String>,
    pub expected_encrypted_auth_config: Option<String>,
    pub encrypted_api_key: Option<String>,
    pub encrypted_auth_config: Option<String>,
}

impl std::fmt::Debug for ProviderCatalogKeyCredentialsCasUpdate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderCatalogKeyCredentialsCasUpdate")
            .field("key_id", &self.key_id)
            .field("expected_provider_id", &self.expected_provider_id)
            .field(
                "expected_encrypted_api_key",
                &redacted_debug_option(&self.expected_encrypted_api_key),
            )
            .field(
                "expected_encrypted_auth_config",
                &redacted_debug_option(&self.expected_encrypted_auth_config),
            )
            .field(
                "encrypted_api_key",
                &redacted_debug_option(&self.encrypted_api_key),
            )
            .field(
                "encrypted_auth_config",
                &redacted_debug_option(&self.encrypted_auth_config),
            )
            .finish()
    }
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredProviderCatalogProvider {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub website: Option<String>,
    pub provider_type: String,
    pub billing_type: Option<String>,
    pub monthly_quota_usd: Option<f64>,
    pub monthly_used_usd: Option<f64>,
    pub quota_reset_day: Option<u64>,
    pub quota_last_reset_at_unix_secs: Option<u64>,
    pub quota_expires_at_unix_secs: Option<u64>,
    pub provider_priority: i32,
    pub is_active: bool,
    pub keep_priority_on_conversion: bool,
    pub enable_format_conversion: bool,
    pub concurrent_limit: Option<i32>,
    pub max_retries: Option<i32>,
    pub proxy: Option<serde_json::Value>,
    pub request_timeout_secs: Option<f64>,
    pub stream_first_byte_timeout_secs: Option<f64>,
    pub config: Option<serde_json::Value>,
    pub created_at_unix_ms: Option<u64>,
    pub updated_at_unix_secs: Option<u64>,
}

impl std::fmt::Debug for StoredProviderCatalogProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredProviderCatalogProvider")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("provider_type", &self.provider_type)
            .field("billing_type", &self.billing_type)
            .field("is_active", &self.is_active)
            .field("proxy", &redacted_debug_option(&self.proxy))
            .field("config", &redacted_debug_option(&self.config))
            .field("created_at_unix_ms", &self.created_at_unix_ms)
            .field("updated_at_unix_secs", &self.updated_at_unix_secs)
            .finish_non_exhaustive()
    }
}

impl StoredProviderCatalogProvider {
    pub fn new(
        id: String,
        name: String,
        website: Option<String>,
        provider_type: String,
    ) -> Result<Self, crate::DataLayerError> {
        if name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "providers.name is empty".to_string(),
            ));
        }
        if provider_type.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "providers.provider_type is empty".to_string(),
            ));
        }

        Ok(Self {
            id,
            name,
            description: None,
            website,
            provider_type,
            billing_type: None,
            monthly_quota_usd: None,
            monthly_used_usd: None,
            quota_reset_day: None,
            quota_last_reset_at_unix_secs: None,
            quota_expires_at_unix_secs: None,
            provider_priority: 0,
            is_active: true,
            keep_priority_on_conversion: false,
            enable_format_conversion: false,
            concurrent_limit: None,
            max_retries: None,
            proxy: None,
            request_timeout_secs: None,
            stream_first_byte_timeout_secs: None,
            config: None,
            created_at_unix_ms: None,
            updated_at_unix_secs: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_transport_fields(
        mut self,
        is_active: bool,
        keep_priority_on_conversion: bool,
        enable_format_conversion: bool,
        concurrent_limit: Option<i32>,
        max_retries: Option<i32>,
        proxy: Option<serde_json::Value>,
        request_timeout_secs: Option<f64>,
        stream_first_byte_timeout_secs: Option<f64>,
        config: Option<serde_json::Value>,
    ) -> Self {
        self.is_active = is_active;
        self.keep_priority_on_conversion = keep_priority_on_conversion;
        self.enable_format_conversion = enable_format_conversion;
        self.concurrent_limit = concurrent_limit;
        self.max_retries = max_retries;
        self.proxy = proxy;
        self.request_timeout_secs = request_timeout_secs;
        self.stream_first_byte_timeout_secs = stream_first_byte_timeout_secs;
        self.config = config;
        self
    }

    pub fn with_description(mut self, description: Option<String>) -> Self {
        self.description = description;
        self
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_billing_fields(
        mut self,
        billing_type: Option<String>,
        monthly_quota_usd: Option<f64>,
        monthly_used_usd: Option<f64>,
        quota_reset_day: Option<u64>,
        quota_last_reset_at_unix_secs: Option<u64>,
        quota_expires_at_unix_secs: Option<u64>,
    ) -> Self {
        self.billing_type = billing_type;
        self.monthly_quota_usd = monthly_quota_usd;
        self.monthly_used_usd = monthly_used_usd;
        self.quota_reset_day = quota_reset_day;
        self.quota_last_reset_at_unix_secs = quota_last_reset_at_unix_secs;
        self.quota_expires_at_unix_secs = quota_expires_at_unix_secs;
        self
    }

    pub fn with_routing_fields(mut self, provider_priority: i32) -> Self {
        self.provider_priority = provider_priority;
        self
    }

    pub fn with_timestamps(
        mut self,
        created_at_unix_ms: Option<u64>,
        updated_at_unix_secs: Option<u64>,
    ) -> Self {
        self.created_at_unix_ms = created_at_unix_ms;
        self.updated_at_unix_secs = updated_at_unix_secs;
        self
    }
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredProviderCatalogEndpoint {
    pub id: String,
    pub provider_id: String,
    pub api_format: String,
    pub api_family: Option<String>,
    pub endpoint_kind: Option<String>,
    pub is_active: bool,
    pub health_score: f64,
    pub base_url: String,
    pub header_rules: Option<serde_json::Value>,
    pub body_rules: Option<serde_json::Value>,
    pub max_retries: Option<i32>,
    pub custom_path: Option<String>,
    pub config: Option<serde_json::Value>,
    pub format_acceptance_config: Option<serde_json::Value>,
    pub proxy: Option<serde_json::Value>,
    pub created_at_unix_ms: Option<u64>,
    pub updated_at_unix_secs: Option<u64>,
}

impl std::fmt::Debug for StoredProviderCatalogEndpoint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredProviderCatalogEndpoint")
            .field("id", &self.id)
            .field("provider_id", &self.provider_id)
            .field("api_format", &self.api_format)
            .field("api_family", &self.api_family)
            .field("endpoint_kind", &self.endpoint_kind)
            .field("is_active", &self.is_active)
            .field("base_url", &REDACTED_DEBUG_VALUE)
            .field("header_rules", &redacted_debug_option(&self.header_rules))
            .field("body_rules", &redacted_debug_option(&self.body_rules))
            .field("config", &redacted_debug_option(&self.config))
            .field("proxy", &redacted_debug_option(&self.proxy))
            .field("created_at_unix_ms", &self.created_at_unix_ms)
            .field("updated_at_unix_secs", &self.updated_at_unix_secs)
            .finish_non_exhaustive()
    }
}

impl StoredProviderCatalogEndpoint {
    pub fn new(
        id: String,
        provider_id: String,
        api_format: String,
        api_family: Option<String>,
        endpoint_kind: Option<String>,
        is_active: bool,
    ) -> Result<Self, crate::DataLayerError> {
        if api_format.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "provider_endpoints.api_format is empty".to_string(),
            ));
        }

        Ok(Self {
            id,
            provider_id,
            api_format,
            api_family,
            endpoint_kind,
            is_active,
            health_score: 1.0,
            base_url: String::new(),
            header_rules: None,
            body_rules: None,
            max_retries: None,
            custom_path: None,
            config: None,
            format_acceptance_config: None,
            proxy: None,
            created_at_unix_ms: None,
            updated_at_unix_secs: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_transport_fields(
        mut self,
        base_url: String,
        header_rules: Option<serde_json::Value>,
        body_rules: Option<serde_json::Value>,
        max_retries: Option<i32>,
        custom_path: Option<String>,
        config: Option<serde_json::Value>,
        format_acceptance_config: Option<serde_json::Value>,
        proxy: Option<serde_json::Value>,
    ) -> Result<Self, crate::DataLayerError> {
        if base_url.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "provider_endpoints.base_url is empty".to_string(),
            ));
        }

        self.base_url = base_url;
        self.header_rules = header_rules;
        self.body_rules = body_rules;
        self.max_retries = max_retries;
        self.custom_path = custom_path;
        self.config = config;
        self.format_acceptance_config = format_acceptance_config;
        self.proxy = proxy;
        Ok(self)
    }

    pub fn with_health_score(mut self, health_score: f64) -> Self {
        self.health_score = health_score;
        self
    }

    pub fn with_timestamps(
        mut self,
        created_at_unix_ms: Option<u64>,
        updated_at_unix_secs: Option<u64>,
    ) -> Self {
        self.created_at_unix_ms = created_at_unix_ms;
        self.updated_at_unix_secs = updated_at_unix_secs;
        self
    }
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredProviderCatalogKey {
    pub id: String,
    pub provider_id: String,
    pub name: String,
    pub auth_type: String,
    pub capabilities: Option<serde_json::Value>,
    pub is_active: bool,
    pub api_formats: Option<serde_json::Value>,
    pub auth_type_by_format: Option<serde_json::Value>,
    pub allow_auth_channel_mismatch_formats: Option<serde_json::Value>,
    pub encrypted_api_key: Option<String>,
    pub encrypted_auth_config: Option<String>,
    pub note: Option<String>,
    pub internal_priority: i32,
    pub rate_multipliers: Option<serde_json::Value>,
    pub global_priority_by_format: Option<serde_json::Value>,
    pub allowed_models: Option<serde_json::Value>,
    pub expires_at_unix_secs: Option<u64>,
    pub cache_ttl_minutes: i32,
    pub max_probe_interval_minutes: i32,
    pub proxy: Option<serde_json::Value>,
    pub fingerprint: Option<serde_json::Value>,
    pub rpm_limit: Option<u32>,
    pub concurrent_limit: Option<i32>,
    pub learned_rpm_limit: Option<u32>,
    pub concurrent_429_count: Option<u32>,
    pub rpm_429_count: Option<u32>,
    pub last_429_at_unix_secs: Option<u64>,
    pub last_429_type: Option<String>,
    pub adjustment_history: Option<serde_json::Value>,
    pub utilization_samples: Option<serde_json::Value>,
    pub last_probe_increase_at_unix_secs: Option<u64>,
    pub last_rpm_peak: Option<u32>,
    pub request_count: Option<u32>,
    pub total_tokens: u64,
    pub total_cost_usd: f64,
    pub success_count: Option<u32>,
    pub error_count: Option<u32>,
    pub total_response_time_ms: Option<u64>,
    pub last_used_at_unix_secs: Option<u64>,
    pub auto_fetch_models: bool,
    pub last_models_fetch_at_unix_secs: Option<u64>,
    pub last_models_fetch_error: Option<String>,
    pub locked_models: Option<serde_json::Value>,
    pub model_include_patterns: Option<serde_json::Value>,
    pub model_exclude_patterns: Option<serde_json::Value>,
    pub upstream_metadata: Option<serde_json::Value>,
    pub oauth_invalid_at_unix_secs: Option<u64>,
    pub oauth_invalid_reason: Option<String>,
    pub status_snapshot: Option<serde_json::Value>,
    pub created_at_unix_ms: Option<u64>,
    pub updated_at_unix_secs: Option<u64>,
    pub health_by_format: Option<serde_json::Value>,
    pub circuit_breaker_by_format: Option<serde_json::Value>,
}

impl std::fmt::Debug for StoredProviderCatalogKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredProviderCatalogKey")
            .field("id", &self.id)
            .field("provider_id", &self.provider_id)
            .field("name", &self.name)
            .field("auth_type", &self.auth_type)
            .field("is_active", &self.is_active)
            .field(
                "encrypted_api_key",
                &redacted_debug_option(&self.encrypted_api_key),
            )
            .field(
                "encrypted_auth_config",
                &redacted_debug_option(&self.encrypted_auth_config),
            )
            .field("proxy", &redacted_debug_option(&self.proxy))
            .field("fingerprint", &redacted_debug_option(&self.fingerprint))
            .field(
                "upstream_metadata",
                &redacted_debug_option(&self.upstream_metadata),
            )
            .field(
                "oauth_invalid_reason",
                &redacted_debug_option(&self.oauth_invalid_reason),
            )
            .field(
                "status_snapshot",
                &redacted_debug_option(&self.status_snapshot),
            )
            .field("created_at_unix_ms", &self.created_at_unix_ms)
            .field("updated_at_unix_secs", &self.updated_at_unix_secs)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq)]
pub struct StoredProviderCatalogKeyMaintenanceSummary {
    pub id: String,
    pub provider_id: String,
    pub is_active: bool,
    pub upstream_metadata: Option<serde_json::Value>,
}

impl std::fmt::Debug for StoredProviderCatalogKeyMaintenanceSummary {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredProviderCatalogKeyMaintenanceSummary")
            .field("id", &self.id)
            .field("provider_id", &self.provider_id)
            .field("is_active", &self.is_active)
            .field(
                "upstream_metadata",
                &redacted_debug_option(&self.upstream_metadata),
            )
            .finish()
    }
}

impl StoredProviderCatalogKey {
    pub fn new(
        id: String,
        provider_id: String,
        name: String,
        auth_type: String,
        capabilities: Option<serde_json::Value>,
        is_active: bool,
    ) -> Result<Self, crate::DataLayerError> {
        if name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "provider_api_keys.name is empty".to_string(),
            ));
        }
        if auth_type.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "provider_api_keys.auth_type is empty".to_string(),
            ));
        }

        Ok(Self {
            id,
            provider_id,
            name,
            auth_type,
            capabilities,
            is_active,
            api_formats: None,
            auth_type_by_format: None,
            allow_auth_channel_mismatch_formats: None,
            encrypted_api_key: None,
            encrypted_auth_config: None,
            note: None,
            internal_priority: 50,
            rate_multipliers: None,
            global_priority_by_format: None,
            allowed_models: None,
            expires_at_unix_secs: None,
            cache_ttl_minutes: 5,
            max_probe_interval_minutes: 32,
            proxy: None,
            fingerprint: None,
            rpm_limit: None,
            concurrent_limit: None,
            learned_rpm_limit: None,
            concurrent_429_count: None,
            rpm_429_count: None,
            last_429_at_unix_secs: None,
            last_429_type: None,
            adjustment_history: None,
            utilization_samples: None,
            last_probe_increase_at_unix_secs: None,
            last_rpm_peak: None,
            request_count: None,
            total_tokens: 0,
            total_cost_usd: 0.0,
            success_count: None,
            error_count: None,
            total_response_time_ms: None,
            last_used_at_unix_secs: None,
            auto_fetch_models: false,
            last_models_fetch_at_unix_secs: None,
            last_models_fetch_error: None,
            locked_models: None,
            model_include_patterns: None,
            model_exclude_patterns: None,
            upstream_metadata: None,
            oauth_invalid_at_unix_secs: None,
            oauth_invalid_reason: None,
            status_snapshot: None,
            created_at_unix_ms: None,
            updated_at_unix_secs: None,
            health_by_format: None,
            circuit_breaker_by_format: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_transport_fields(
        mut self,
        api_formats: Option<serde_json::Value>,
        encrypted_api_key: impl Into<Option<String>>,
        encrypted_auth_config: Option<String>,
        rate_multipliers: Option<serde_json::Value>,
        global_priority_by_format: Option<serde_json::Value>,
        allowed_models: Option<serde_json::Value>,
        expires_at_unix_secs: Option<u64>,
        proxy: Option<serde_json::Value>,
        fingerprint: Option<serde_json::Value>,
    ) -> Result<Self, crate::DataLayerError> {
        let encrypted_api_key = encrypted_api_key.into();
        if encrypted_api_key
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(crate::DataLayerError::UnexpectedValue(
                "provider_api_keys.api_key is empty".to_string(),
            ));
        }

        self.api_formats = api_formats;
        self.encrypted_api_key = encrypted_api_key;
        self.encrypted_auth_config = encrypted_auth_config;
        self.rate_multipliers = rate_multipliers;
        self.global_priority_by_format = global_priority_by_format;
        self.allowed_models = allowed_models;
        self.expires_at_unix_secs = expires_at_unix_secs;
        self.proxy = proxy;
        self.fingerprint = fingerprint;
        Ok(self)
    }

    pub fn with_auth_channel_policy_fields(
        mut self,
        auth_type_by_format: Option<serde_json::Value>,
        allow_auth_channel_mismatch_formats: Option<serde_json::Value>,
    ) -> Result<Self, crate::DataLayerError> {
        validate_auth_type_by_format(auth_type_by_format.as_ref())?;
        validate_auth_channel_mismatch_formats(allow_auth_channel_mismatch_formats.as_ref())?;
        self.auth_type_by_format = auth_type_by_format;
        self.allow_auth_channel_mismatch_formats = allow_auth_channel_mismatch_formats;
        Ok(self)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_rate_limit_fields(
        mut self,
        rpm_limit: Option<u32>,
        concurrent_limit: Option<i32>,
        learned_rpm_limit: Option<u32>,
        concurrent_429_count: Option<u32>,
        rpm_429_count: Option<u32>,
        last_429_at_unix_secs: Option<u64>,
        adjustment_history: Option<serde_json::Value>,
        request_count: Option<u32>,
        success_count: Option<u32>,
    ) -> Self {
        self.rpm_limit = rpm_limit;
        self.concurrent_limit = concurrent_limit;
        self.learned_rpm_limit = learned_rpm_limit;
        self.concurrent_429_count = concurrent_429_count;
        self.rpm_429_count = rpm_429_count;
        self.last_429_at_unix_secs = last_429_at_unix_secs;
        self.adjustment_history = adjustment_history;
        self.request_count = request_count;
        self.success_count = success_count;
        self
    }

    pub fn with_usage_fields(
        mut self,
        error_count: Option<u32>,
        total_response_time_ms: Option<u64>,
    ) -> Self {
        self.error_count = error_count;
        self.total_response_time_ms = total_response_time_ms;
        self
    }

    pub fn with_usage_totals(mut self, total_tokens: u64, total_cost_usd: f64) -> Self {
        self.total_tokens = total_tokens;
        self.total_cost_usd = if total_cost_usd.is_finite() {
            total_cost_usd
        } else {
            0.0
        };
        self
    }

    pub fn with_health_fields(
        mut self,
        health_by_format: Option<serde_json::Value>,
        circuit_breaker_by_format: Option<serde_json::Value>,
    ) -> Self {
        self.health_by_format = health_by_format;
        self.circuit_breaker_by_format = circuit_breaker_by_format;
        self
    }
}

fn validate_auth_type_by_format(
    value: Option<&serde_json::Value>,
) -> Result<(), crate::DataLayerError> {
    let Some(value) = value else {
        return Ok(());
    };
    let Some(entries) = value.as_object() else {
        return Err(crate::DataLayerError::UnexpectedValue(
            "provider_api_keys.auth_type_by_format must be a JSON object".to_string(),
        ));
    };
    for (api_format, auth_type) in entries {
        let valid_api_format = !api_format.trim().is_empty();
        let valid_auth_type = auth_type.as_str().is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "api_key" | "bearer"
            )
        });
        if !valid_api_format || !valid_auth_type {
            return Err(crate::DataLayerError::UnexpectedValue(
                "provider_api_keys.auth_type_by_format contains an invalid entry".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_auth_channel_mismatch_formats(
    value: Option<&serde_json::Value>,
) -> Result<(), crate::DataLayerError> {
    let Some(value) = value else {
        return Ok(());
    };
    let Some(items) = value.as_array() else {
        return Err(crate::DataLayerError::UnexpectedValue(
            "provider_api_keys.allow_auth_channel_mismatch_formats must be a JSON array"
                .to_string(),
        ));
    };
    if items
        .iter()
        .any(|item| item.as_str().is_none_or(|value| value.trim().is_empty()))
    {
        return Err(crate::DataLayerError::UnexpectedValue(
            "provider_api_keys.allow_auth_channel_mismatch_formats contains an invalid entry"
                .to_string(),
        ));
    }
    Ok(())
}

impl From<&StoredProviderCatalogKey> for ProviderCatalogKeyAdaptiveState {
    fn from(key: &StoredProviderCatalogKey) -> Self {
        Self {
            learned_rpm_limit: key.learned_rpm_limit,
            concurrent_429_count: Some(key.concurrent_429_count.unwrap_or(0)),
            rpm_429_count: Some(key.rpm_429_count.unwrap_or(0)),
            last_429_at_unix_secs: key.last_429_at_unix_secs,
            last_429_type: key.last_429_type.clone(),
            adjustment_history: key.adjustment_history.clone(),
            utilization_samples: key.utilization_samples.clone(),
            last_probe_increase_at_unix_secs: key.last_probe_increase_at_unix_secs,
            last_rpm_peak: key.last_rpm_peak,
        }
    }
}

#[cfg(test)]
mod transport_tests {
    use super::{
        ProviderCatalogKeyCredentialsCasUpdate, ProviderCatalogKeyOAuthCredentialFence,
        ProviderCatalogKeyOAuthRuntimeStateCasUpdate,
        ProviderCatalogUpstreamMetadataNamespaceExpectation, StoredProviderCatalogEndpoint,
        StoredProviderCatalogKey, StoredProviderCatalogProvider,
    };

    fn assert_debug_redacts<T: std::fmt::Debug>(value: &T, secrets: &[&str]) {
        let debug = format!("{value:?}");
        assert!(debug.contains("[REDACTED]"), "debug output: {debug}");
        for secret in secrets {
            assert!(
                !debug.contains(secret),
                "debug output leaked {secret}: {debug}"
            );
        }
    }

    fn sample_key() -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            "key-policy".to_string(),
            "provider-policy".to_string(),
            "policy".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build")
    }

    #[test]
    fn provider_catalog_key_auth_channel_policy_rejects_malformed_stored_json() {
        assert!(sample_key()
            .with_auth_channel_policy_fields(
                Some(serde_json::json!({"openai:chat": "bearer"})),
                Some(serde_json::json!([])),
            )
            .is_ok());
        assert!(sample_key()
            .with_auth_channel_policy_fields(Some(serde_json::Value::Null), None)
            .is_err());
        assert!(sample_key()
            .with_auth_channel_policy_fields(
                Some(serde_json::json!({"openai:chat": "oauth"})),
                None,
            )
            .is_err());
        assert!(sample_key()
            .with_auth_channel_policy_fields(None, Some(serde_json::Value::Null))
            .is_err());
        assert!(sample_key()
            .with_auth_channel_policy_fields(None, Some(serde_json::json!([""])))
            .is_err());
    }

    #[test]
    fn provider_catalog_key_defaults_concurrent_limit_to_none() {
        let key = StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "default".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build");

        assert_eq!(key.concurrent_limit, None);
    }

    #[test]
    fn provider_catalog_key_rate_limit_builder_sets_concurrent_limit() {
        let key = StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "default".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_rate_limit_fields(
            Some(120),
            Some(3),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );

        assert_eq!(key.rpm_limit, Some(120));
        assert_eq!(key.concurrent_limit, Some(3));
    }

    #[test]
    fn provider_catalog_debug_output_redacts_credentials_and_transport_metadata() {
        let mut key = sample_key();
        key.encrypted_api_key = Some("catalog-api-key-ciphertext-canary".to_string());
        key.encrypted_auth_config = Some("catalog-auth-config-ciphertext-canary".to_string());
        key.proxy = Some(serde_json::json!({"password": "catalog-proxy-canary"}));
        key.fingerprint = Some(serde_json::json!({"device": "catalog-device-canary"}));
        key.upstream_metadata = Some(serde_json::json!({"token": "catalog-metadata-canary"}));
        key.oauth_invalid_reason = Some("catalog-oauth-reason-canary".to_string());
        key.status_snapshot = Some(serde_json::json!({"raw": "catalog-status-canary"}));
        assert_debug_redacts(
            &key,
            &[
                "catalog-api-key-ciphertext-canary",
                "catalog-auth-config-ciphertext-canary",
                "catalog-proxy-canary",
                "catalog-device-canary",
                "catalog-metadata-canary",
                "catalog-oauth-reason-canary",
                "catalog-status-canary",
            ],
        );

        let provider = StoredProviderCatalogProvider::new(
            "provider-debug".to_string(),
            "debug".to_string(),
            None,
            "openai".to_string(),
        )
        .expect("provider should build")
        .with_transport_fields(
            true,
            false,
            false,
            None,
            None,
            Some(serde_json::json!({"password": "provider-proxy-canary"})),
            None,
            None,
            Some(serde_json::json!({"secret": "provider-config-canary"})),
        );
        assert_debug_redacts(
            &provider,
            &["provider-proxy-canary", "provider-config-canary"],
        );

        let endpoint = StoredProviderCatalogEndpoint::new(
            "endpoint-debug".to_string(),
            "provider-debug".to_string(),
            "openai:chat".to_string(),
            None,
            None,
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://endpoint-user:endpoint-password-canary@example.com/endpoint-token-canary"
                .to_string(),
            Some(serde_json::json!({"Authorization": "endpoint-header-canary"})),
            Some(serde_json::json!({"credential": "endpoint-body-canary"})),
            None,
            None,
            Some(serde_json::json!({"secret": "endpoint-config-canary"})),
            None,
            Some(serde_json::json!({"password": "endpoint-proxy-canary"})),
        )
        .expect("endpoint should accept transport fields");
        assert_debug_redacts(
            &endpoint,
            &[
                "endpoint-password-canary",
                "endpoint-token-canary",
                "endpoint-header-canary",
                "endpoint-body-canary",
                "endpoint-config-canary",
                "endpoint-proxy-canary",
            ],
        );
    }

    #[test]
    fn provider_catalog_cas_debug_output_redacts_credential_fences() {
        let fence = ProviderCatalogKeyOAuthCredentialFence {
            encrypted_api_key: Some("fence-api-key-canary".to_string()),
            auth_type: "oauth".to_string(),
            provider_id: "provider-debug".to_string(),
            provider_type: "codex".to_string(),
        };
        assert_debug_redacts(&fence, &["fence-api-key-canary"]);

        let credentials = ProviderCatalogKeyCredentialsCasUpdate {
            key_id: "key-debug".to_string(),
            expected_provider_id: "provider-debug".to_string(),
            expected_encrypted_api_key: Some("expected-api-key-canary".to_string()),
            expected_encrypted_auth_config: Some("expected-auth-config-canary".to_string()),
            encrypted_api_key: Some("replacement-api-key-canary".to_string()),
            encrypted_auth_config: Some("replacement-auth-config-canary".to_string()),
        };
        assert_debug_redacts(
            &credentials,
            &[
                "expected-api-key-canary",
                "expected-auth-config-canary",
                "replacement-api-key-canary",
                "replacement-auth-config-canary",
            ],
        );

        let runtime = ProviderCatalogKeyOAuthRuntimeStateCasUpdate {
            key_id: "key-debug".to_string(),
            expected_encrypted_auth_config: Some("runtime-expected-auth-canary".to_string()),
            expected_credential: Some(fence),
            expected_upstream_metadata_namespace: Some(
                ProviderCatalogUpstreamMetadataNamespaceExpectation {
                    namespace: "oauth".to_string(),
                    expected_value: Some(serde_json::json!({"token": "runtime-fence-canary"})),
                },
            ),
            encrypted_auth_config: "runtime-auth-config-canary".to_string(),
            encrypted_api_key_update: Some("runtime-api-key-canary".to_string()),
            expires_at_unix_secs_update: Some(Some(123)),
            oauth_invalid_at_unix_secs: Some(124),
            oauth_invalid_reason: Some("runtime-provider-error-canary".to_string()),
            upstream_metadata_patch: Some(serde_json::json!({
                "refresh_token": "runtime-metadata-canary"
            })),
            upstream_metadata_namespace_to_remove: None,
            status_snapshot_patch: serde_json::json!({"raw": "runtime-status-canary"}),
            reset_error_count: true,
            updated_at_unix_secs: Some(125),
        };
        assert_debug_redacts(
            &runtime,
            &[
                "runtime-expected-auth-canary",
                "fence-api-key-canary",
                "runtime-fence-canary",
                "runtime-auth-config-canary",
                "runtime-api-key-canary",
                "runtime-provider-error-canary",
                "runtime-metadata-canary",
                "runtime-status-canary",
            ],
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ProviderCatalogKeyListOrder {
    #[default]
    Name,
    CreatedAt,
    CreatedAtAsc,
    CreatedAtDesc,
    LastUsedAtAsc,
    LastUsedAtDesc,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProviderCatalogKeyListQuery {
    pub provider_id: String,
    pub search: Option<String>,
    pub is_active: Option<bool>,
    pub offset: usize,
    pub limit: usize,
    pub order: ProviderCatalogKeyListOrder,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredProviderCatalogKeyPage {
    pub items: Vec<StoredProviderCatalogKey>,
    pub total: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredProviderCatalogKeyStats {
    pub provider_id: String,
    pub total_keys: u64,
    pub active_keys: u64,
}

impl StoredProviderCatalogKeyStats {
    pub fn new(
        provider_id: String,
        total_keys: i64,
        active_keys: i64,
    ) -> Result<Self, crate::DataLayerError> {
        if provider_id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "provider key stats provider_id is empty".to_string(),
            ));
        }
        if total_keys < 0 || active_keys < 0 {
            return Err(crate::DataLayerError::UnexpectedValue(
                "provider key stats count is negative".to_string(),
            ));
        }

        Ok(Self {
            provider_id,
            total_keys: total_keys as u64,
            active_keys: active_keys as u64,
        })
    }
}

#[async_trait]
pub trait ProviderCatalogReadRepository: Send + Sync {
    fn clear_local_cache(&self) {}

    async fn list_providers(
        &self,
        active_only: bool,
    ) -> Result<Vec<StoredProviderCatalogProvider>, crate::DataLayerError>;

    async fn list_providers_by_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogProvider>, crate::DataLayerError>;

    async fn list_endpoints_by_ids(
        &self,
        endpoint_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogEndpoint>, crate::DataLayerError>;

    async fn list_endpoints_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogEndpoint>, crate::DataLayerError>;

    async fn list_keys_by_ids(
        &self,
        key_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, crate::DataLayerError>;

    /// Reads credential-bearing key records without an intervening repository cache.
    ///
    /// Callers use this only for security-sensitive generation fences where a short-lived cached
    /// key record could bind data to credentials that an administrator has already replaced.
    /// Repository implementations that do not add a read cache can use the default behavior.
    async fn list_keys_by_ids_strong(
        &self,
        key_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, crate::DataLayerError> {
        self.list_keys_by_ids(key_ids).await
    }

    async fn list_keys_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, crate::DataLayerError>;

    async fn list_key_summaries_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, crate::DataLayerError>;

    async fn list_key_maintenance_summaries_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKeyMaintenanceSummary>, crate::DataLayerError>;

    async fn list_keys_page(
        &self,
        query: &ProviderCatalogKeyListQuery,
    ) -> Result<StoredProviderCatalogKeyPage, crate::DataLayerError>;

    async fn list_key_stats_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKeyStats>, crate::DataLayerError>;
}

#[async_trait]
pub trait ProviderCatalogWriteRepository: Send + Sync {
    async fn create_provider(
        &self,
        provider: &StoredProviderCatalogProvider,
        shift_existing_priorities_from: Option<i32>,
    ) -> Result<StoredProviderCatalogProvider, crate::DataLayerError>;

    async fn update_provider(
        &self,
        provider: &StoredProviderCatalogProvider,
    ) -> Result<StoredProviderCatalogProvider, crate::DataLayerError>;

    async fn compare_and_swap_provider_config(
        &self,
        _update: &ProviderCatalogProviderConfigCasUpdate,
    ) -> Result<bool, crate::DataLayerError> {
        Err(crate::DataLayerError::InvalidConfiguration(
            "provider catalog config compare-and-swap is not supported by this repository"
                .to_string(),
        ))
    }

    async fn compare_and_swap_provider_proxy(
        &self,
        _update: &ProviderCatalogProxyCasUpdate,
    ) -> Result<bool, crate::DataLayerError> {
        Err(crate::DataLayerError::InvalidConfiguration(
            "provider catalog provider proxy compare-and-swap is not supported by this repository"
                .to_string(),
        ))
    }

    async fn delete_provider(&self, provider_id: &str) -> Result<bool, crate::DataLayerError>;

    async fn cleanup_deleted_provider_refs(
        &self,
        provider_id: &str,
        provider_deleted: bool,
        endpoint_ids: &[String],
        key_ids: &[String],
    ) -> Result<(), crate::DataLayerError>;

    async fn create_endpoint(
        &self,
        endpoint: &StoredProviderCatalogEndpoint,
    ) -> Result<StoredProviderCatalogEndpoint, crate::DataLayerError>;

    async fn update_endpoint(
        &self,
        endpoint: &StoredProviderCatalogEndpoint,
    ) -> Result<StoredProviderCatalogEndpoint, crate::DataLayerError>;

    async fn compare_and_swap_endpoint_proxy(
        &self,
        _update: &ProviderCatalogProxyCasUpdate,
    ) -> Result<bool, crate::DataLayerError> {
        Err(crate::DataLayerError::InvalidConfiguration(
            "provider catalog endpoint proxy compare-and-swap is not supported by this repository"
                .to_string(),
        ))
    }

    async fn delete_endpoint(&self, endpoint_id: &str) -> Result<bool, crate::DataLayerError>;

    async fn create_key(
        &self,
        key: &StoredProviderCatalogKey,
    ) -> Result<StoredProviderCatalogKey, crate::DataLayerError>;

    async fn update_key(
        &self,
        key: &StoredProviderCatalogKey,
    ) -> Result<StoredProviderCatalogKey, crate::DataLayerError>;

    async fn compare_and_swap_key_proxy(
        &self,
        _update: &ProviderCatalogProxyCasUpdate,
    ) -> Result<bool, crate::DataLayerError> {
        Err(crate::DataLayerError::InvalidConfiguration(
            "provider catalog key proxy compare-and-swap is not supported by this repository"
                .to_string(),
        ))
    }

    async fn compare_and_swap_key_credentials(
        &self,
        _update: &ProviderCatalogKeyCredentialsCasUpdate,
    ) -> Result<bool, crate::DataLayerError> {
        Err(crate::DataLayerError::InvalidConfiguration(
            "provider catalog key credential compare-and-swap is not supported by this repository"
                .to_string(),
        ))
    }

    /// Compare-and-swap administrator-owned key configuration. Credential
    /// rotation, Codex namespace replacement, and quota invalidation must be
    /// committed atomically with the configuration update.
    async fn compare_and_update_key_admin_state(
        &self,
        _update: &ProviderCatalogKeyAdminCasUpdate,
    ) -> Result<bool, crate::DataLayerError> {
        Err(crate::DataLayerError::InvalidConfiguration(
            "provider catalog admin CAS updates are not supported by this repository".to_string(),
        ))
    }

    async fn update_keys(
        &self,
        keys: &[StoredProviderCatalogKey],
    ) -> Result<Vec<StoredProviderCatalogKey>, crate::DataLayerError>;

    async fn update_key_upstream_metadata(
        &self,
        key_id: &str,
        upstream_metadata: Option<&serde_json::Value>,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<bool, crate::DataLayerError>;

    async fn upsert_key_upstream_metadata_namespace(
        &self,
        key_id: &str,
        namespace: &str,
        value: &serde_json::Value,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<bool, crate::DataLayerError>;

    async fn update_key_model_fetch_state(
        &self,
        key_id: &str,
        allowed_models: Option<&serde_json::Value>,
        last_models_fetch_at_unix_secs: Option<u64>,
        last_models_fetch_error: Option<&str>,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<bool, crate::DataLayerError>;

    async fn update_key_model_fetch_success(
        &self,
        key_id: &str,
        allowed_models: Option<&serde_json::Value>,
        last_models_fetch_at_unix_secs: u64,
        upstream_metadata_updates: &[ProviderCatalogUpstreamMetadataNamespaceUpdate],
        updated_at_unix_secs: Option<u64>,
    ) -> Result<bool, crate::DataLayerError>;

    async fn delete_key(&self, key_id: &str) -> Result<bool, crate::DataLayerError>;

    async fn compare_and_delete_key_oauth_credential(
        &self,
        _delete: &ProviderCatalogKeyOAuthCredentialCasDelete,
    ) -> Result<bool, crate::DataLayerError> {
        Err(crate::DataLayerError::InvalidConfiguration(
            "provider catalog OAuth credential CAS deletes are not supported by this repository"
                .to_string(),
        ))
    }

    async fn clear_key_oauth_invalid_marker(
        &self,
        key_id: &str,
    ) -> Result<bool, crate::DataLayerError>;

    async fn update_key_oauth_runtime_state(
        &self,
        key_id: &str,
        oauth_invalid_at_unix_secs: Option<u64>,
        oauth_invalid_reason: Option<&str>,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<bool, crate::DataLayerError>;

    /// Compare-and-swap OAuth runtime credentials/status without replacing any
    /// administrator-owned key fields.
    async fn compare_and_update_key_oauth_runtime_state(
        &self,
        _update: &ProviderCatalogKeyOAuthRuntimeStateCasUpdate,
    ) -> Result<bool, crate::DataLayerError> {
        Err(crate::DataLayerError::InvalidConfiguration(
            "provider catalog OAuth runtime CAS updates are not supported by this repository"
                .to_string(),
        ))
    }

    async fn update_key_health_state(
        &self,
        key_id: &str,
        is_active: bool,
        health_by_format: Option<&serde_json::Value>,
        circuit_breaker_by_format: Option<&serde_json::Value>,
    ) -> Result<bool, crate::DataLayerError>;

    /// Explicit administrator recovery action; does not replace other usage counters.
    async fn reset_key_error_count(&self, key_id: &str) -> Result<bool, crate::DataLayerError>;

    /// Compare-and-swap adaptive fields without replacing unrelated key columns.
    async fn compare_and_update_key_adaptive_state(
        &self,
        _update: &ProviderCatalogKeyAdaptiveStateUpdate,
    ) -> Result<bool, crate::DataLayerError> {
        Err(crate::DataLayerError::InvalidConfiguration(
            "provider catalog adaptive state updates are not supported by this repository"
                .to_string(),
        ))
    }

    /// Atomically replaces one upstream metadata namespace and merges owned status fields.
    async fn update_key_runtime_metadata(
        &self,
        _update: &ProviderCatalogKeyRuntimeMetadataUpdate,
    ) -> Result<bool, crate::DataLayerError> {
        Err(crate::DataLayerError::InvalidConfiguration(
            "provider catalog runtime metadata updates are not supported by this repository"
                .to_string(),
        ))
    }

    /// Atomically merges caller-owned top-level status fields.
    async fn update_key_status_snapshot(
        &self,
        _update: &ProviderCatalogKeyStatusSnapshotUpdate,
    ) -> Result<bool, crate::DataLayerError> {
        Err(crate::DataLayerError::InvalidConfiguration(
            "provider catalog status snapshot patches are not supported by this repository"
                .to_string(),
        ))
    }

    /// Compare-and-swap health JSON without changing administrator-owned activation state.
    async fn compare_and_update_key_health_state(
        &self,
        _update: &ProviderCatalogKeyHealthStateUpdate,
    ) -> Result<bool, crate::DataLayerError> {
        Err(crate::DataLayerError::InvalidConfiguration(
            "provider catalog runtime health updates are not supported by this repository"
                .to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::StoredProviderCatalogKey;

    fn sample_key() -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "key".to_string(),
            "service_account".to_string(),
            None,
            true,
        )
        .expect("key should build")
    }

    #[test]
    fn transport_fields_allow_null_encrypted_api_key() {
        let key = sample_key()
            .with_transport_fields(
                None,
                None::<String>,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .expect("null api key should be accepted");

        assert_eq!(key.encrypted_api_key, None);
    }

    #[test]
    fn transport_fields_reject_empty_encrypted_api_key_string() {
        let err = sample_key()
            .with_transport_fields(
                None,
                Some("   ".to_string()),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .expect_err("empty api key string should be rejected");

        assert!(err
            .to_string()
            .contains("provider_api_keys.api_key is empty"));
    }
}
