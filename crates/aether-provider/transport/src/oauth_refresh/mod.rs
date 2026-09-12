use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aether_oauth::core::OAuthError;
use aether_oauth::network::{
    OAuthHttpExecutor, OAuthHttpRequest, OAuthHttpResponse, OAuthNetworkContext,
};
use aether_oauth::provider::ProviderOAuthTransportContext;
use aether_runtime_state::{RuntimeLockLease, RuntimeState};
use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;
use tokio::sync::{Mutex, OwnedMutexGuard};

use super::agent_identity::{is_codex_agent_identity_transport, CodexAgentIdentityRefreshAdapter};
use super::generic_oauth::supports_local_generic_oauth_request_auth_resolution;
pub use super::generic_oauth::GenericOAuthRefreshAdapter;
use super::kiro::{
    supports_local_kiro_request_auth_resolution, KiroOAuthRefreshAdapter, KiroRequestAuth,
};
use super::snapshot::GatewayProviderTransportSnapshot;
use super::vertex::{
    supports_local_vertex_service_account_auth_resolution, VertexServiceAccountRefreshAdapter,
};

const LOCAL_OAUTH_RESPONSE_BODY_LIMIT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum LocalResolvedOAuthRequestAuth {
    #[allow(dead_code)]
    Header {
        name: String,
        value: String,
    },
    Kiro(KiroRequestAuth),
}

impl fmt::Debug for LocalResolvedOAuthRequestAuth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Header { name, .. } => formatter
                .debug_struct("Header")
                .field("name", name)
                .field("value", &"[REDACTED]")
                .finish(),
            Self::Kiro(auth) => formatter.debug_tuple("Kiro").field(auth).finish(),
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct LocalOAuthResolution {
    pub auth: Option<LocalResolvedOAuthRequestAuth>,
    pub refreshed_entry: Option<CachedOAuthEntry>,
    pub refresh_in_flight: bool,
    /// Indicates that a forced caller reused a newer completed refresh rather
    /// than producing a new entry that needs persistence.
    pub reused_refresh: bool,
    /// Held until the caller persists `refreshed_entry`. The lease TTL remains
    /// the cancellation fallback if the caller is dropped.
    pub distributed_lease: Option<RuntimeLockLease>,
    /// Keeps memory-only refreshes singleflight until the caller validates the
    /// credential fence and publishes or discards `refreshed_entry`.
    #[doc(hidden)]
    pub local_refresh_guard: Option<LocalOAuthRefreshCommitGuard>,
}

impl fmt::Debug for LocalOAuthResolution {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalOAuthResolution")
            .field("auth", &self.auth)
            .field("refreshed_entry", &self.refreshed_entry)
            .field("refresh_in_flight", &self.refresh_in_flight)
            .field("reused_refresh", &self.reused_refresh)
            .field("has_distributed_lease", &self.distributed_lease.is_some())
            .field(
                "has_local_refresh_guard",
                &self.local_refresh_guard.is_some(),
            )
            .finish()
    }
}

#[derive(Clone)]
pub struct LocalOAuthRefreshCommitGuard {
    guard: Arc<OwnedMutexGuard<()>>,
}

impl LocalOAuthRefreshCommitGuard {
    fn new(guard: OwnedMutexGuard<()>) -> Self {
        Self {
            guard: Arc::new(guard),
        }
    }
}

impl fmt::Debug for LocalOAuthRefreshCommitGuard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LocalOAuthRefreshCommitGuard")
    }
}

impl PartialEq for LocalOAuthRefreshCommitGuard {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.guard, &other.guard)
    }
}

#[derive(Clone, PartialEq)]
pub struct CachedOAuthEntry {
    pub provider_type: String,
    pub auth_header_name: String,
    pub auth_header_value: String,
    pub expires_at_unix_secs: Option<u64>,
    pub metadata: Option<Value>,
    /// Non-secret fingerprint of the credential/configuration that produced it.
    pub source_fingerprint: Option<String>,
}

impl fmt::Debug for CachedOAuthEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CachedOAuthEntry")
            .field("provider_type", &self.provider_type)
            .field("auth_header_name", &self.auth_header_name)
            .field("auth_header_value", &"[REDACTED]")
            .field("expires_at_unix_secs", &self.expires_at_unix_secs)
            .field("has_metadata", &self.metadata.is_some())
            .field("has_source_fingerprint", &self.source_fingerprint.is_some())
            .finish()
    }
}

#[derive(Clone, PartialEq)]
pub struct LocalOAuthHttpRequest {
    pub request_id: &'static str,
    pub method: reqwest::Method,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub json_body: Option<Value>,
    pub body_bytes: Option<Vec<u8>>,
}

impl fmt::Debug for LocalOAuthHttpRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalOAuthHttpRequest")
            .field("request_id", &self.request_id)
            .field("method", &self.method)
            .field("url", &"[REDACTED]")
            .field("header_names", &self.headers.keys().collect::<Vec<_>>())
            .field("json_body", &self.json_body.as_ref().map(|_| "[REDACTED]"))
            .field("body_bytes_len", &self.body_bytes.as_ref().map(Vec::len))
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct LocalOAuthHttpResponse {
    pub status_code: u16,
    pub body_text: String,
}

impl fmt::Debug for LocalOAuthHttpResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalOAuthHttpResponse")
            .field("status_code", &self.status_code)
            .field("body_bytes_len", &self.body_text.len())
            .finish()
    }
}

#[derive(Error)]
pub enum LocalOAuthRefreshError {
    #[error("{provider_type} oauth refresh request failed")]
    Transport {
        provider_type: &'static str,
        error: reqwest::Error,
    },
    #[error("{provider_type} oauth refresh returned HTTP {status_code}")]
    HttpStatus {
        provider_type: &'static str,
        status_code: u16,
        body_excerpt: String,
    },
    #[error("{provider_type} oauth refresh transport failed: {message}")]
    TransportMessage {
        provider_type: &'static str,
        message: String,
    },
    #[error("{provider_type} oauth refresh returned invalid response: {message}")]
    InvalidResponse {
        provider_type: &'static str,
        message: String,
    },
}

#[async_trait]
pub trait LocalOAuthHttpExecutor: Send + Sync {
    async fn execute(
        &self,
        provider_type: &'static str,
        transport: &GatewayProviderTransportSnapshot,
        request: &LocalOAuthHttpRequest,
    ) -> Result<LocalOAuthHttpResponse, LocalOAuthRefreshError>;
}

#[derive(Debug, Clone)]
pub struct ReqwestLocalOAuthHttpExecutor {
    client: reqwest::Client,
}

impl ReqwestLocalOAuthHttpExecutor {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

impl fmt::Debug for LocalOAuthRefreshError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport { provider_type, .. } => formatter
                .debug_struct("Transport")
                .field("provider_type", provider_type)
                .field("error", &"[REDACTED]")
                .finish(),
            Self::HttpStatus {
                provider_type,
                status_code,
                ..
            } => formatter
                .debug_struct("HttpStatus")
                .field("provider_type", provider_type)
                .field("status_code", status_code)
                .field("body_excerpt", &"[REDACTED]")
                .finish(),
            Self::TransportMessage { provider_type, .. } => formatter
                .debug_struct("TransportMessage")
                .field("provider_type", provider_type)
                .field("message", &"[REDACTED]")
                .finish(),
            Self::InvalidResponse { provider_type, .. } => formatter
                .debug_struct("InvalidResponse")
                .field("provider_type", provider_type)
                .field("message", &"[REDACTED]")
                .finish(),
        }
    }
}

fn validate_local_oauth_request_url(
    provider_type: &'static str,
    raw_url: &str,
) -> Result<url::Url, LocalOAuthRefreshError> {
    let invalid = |message: &'static str| LocalOAuthRefreshError::TransportMessage {
        provider_type,
        message: message.to_string(),
    };
    let url = url::Url::parse(raw_url).map_err(|_| invalid("invalid OAuth endpoint URL"))?;
    if url.host().is_none() || !matches!(url.scheme(), "http" | "https") {
        return Err(invalid(
            "OAuth endpoint URL must use HTTP or HTTPS and include a host",
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid("OAuth endpoint URL must not include credentials"));
    }
    if url.fragment().is_some() {
        return Err(invalid("OAuth endpoint URL must not include a fragment"));
    }
    if !aether_http::is_https_or_loopback_http_url(&url) {
        return Err(invalid("remote OAuth endpoint URL must use HTTPS"));
    }
    Ok(url)
}

#[async_trait]
impl LocalOAuthHttpExecutor for ReqwestLocalOAuthHttpExecutor {
    async fn execute(
        &self,
        provider_type: &'static str,
        _transport: &GatewayProviderTransportSnapshot,
        request: &LocalOAuthHttpRequest,
    ) -> Result<LocalOAuthHttpResponse, LocalOAuthRefreshError> {
        let request_url = validate_local_oauth_request_url(provider_type, request.url.as_str())?;
        let mut builder = self.client.request(request.method.clone(), request_url);
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        if let Some(json_body) = request.json_body.as_ref() {
            builder = builder.json(json_body);
        } else if let Some(body_bytes) = request.body_bytes.as_ref() {
            builder = builder.body(body_bytes.clone());
        }

        let mut response =
            builder
                .send()
                .await
                .map_err(|error| LocalOAuthRefreshError::Transport {
                    provider_type,
                    error,
                })?;
        let status_code = response.status().as_u16();
        if response
            .content_length()
            .is_some_and(|length| length > LOCAL_OAUTH_RESPONSE_BODY_LIMIT_BYTES as u64)
        {
            return Err(local_oauth_response_too_large(provider_type));
        }
        let mut body = Vec::new();
        while let Some(chunk) =
            response
                .chunk()
                .await
                .map_err(|error| LocalOAuthRefreshError::Transport {
                    provider_type,
                    error,
                })?
        {
            if chunk.len() > LOCAL_OAUTH_RESPONSE_BODY_LIMIT_BYTES.saturating_sub(body.len()) {
                return Err(local_oauth_response_too_large(provider_type));
            }
            body.extend_from_slice(&chunk);
        }
        let body_text = String::from_utf8_lossy(&body).to_string();
        Ok(LocalOAuthHttpResponse {
            status_code,
            body_text,
        })
    }
}

fn local_oauth_response_too_large(provider_type: &'static str) -> LocalOAuthRefreshError {
    LocalOAuthRefreshError::InvalidResponse {
        provider_type,
        message: format!("response body exceeds {LOCAL_OAUTH_RESPONSE_BODY_LIMIT_BYTES} bytes"),
    }
}

pub(crate) struct ProviderOAuthLocalHttpExecutor<'a> {
    provider_type: &'static str,
    transport: &'a GatewayProviderTransportSnapshot,
    inner: &'a dyn LocalOAuthHttpExecutor,
}

impl<'a> ProviderOAuthLocalHttpExecutor<'a> {
    pub(crate) fn new(
        provider_type: &'static str,
        transport: &'a GatewayProviderTransportSnapshot,
        inner: &'a dyn LocalOAuthHttpExecutor,
    ) -> Self {
        Self {
            provider_type,
            transport,
            inner,
        }
    }
}

#[async_trait]
impl OAuthHttpExecutor for ProviderOAuthLocalHttpExecutor<'_> {
    async fn execute(&self, request: OAuthHttpRequest) -> Result<OAuthHttpResponse, OAuthError> {
        let response = self
            .inner
            .execute(
                self.provider_type,
                self.transport,
                &LocalOAuthHttpRequest {
                    request_id: "provider-oauth:local-refresh-token",
                    method: request.method,
                    url: request.url,
                    headers: request.headers,
                    json_body: request.json_body,
                    body_bytes: request.body_bytes,
                },
            )
            .await
            .map_err(local_refresh_error_to_oauth_error)?;
        let json_body = serde_json::from_str::<Value>(&response.body_text).ok();
        Ok(OAuthHttpResponse {
            status_code: response.status_code,
            body_text: response.body_text,
            json_body,
        })
    }
}

pub(crate) fn provider_oauth_transport_context_from_snapshot(
    transport: &GatewayProviderTransportSnapshot,
) -> ProviderOAuthTransportContext {
    ProviderOAuthTransportContext {
        provider_id: transport.provider.id.clone(),
        provider_type: transport.provider.provider_type.clone(),
        endpoint_id: Some(transport.endpoint.id.clone()),
        key_id: Some(transport.key.id.clone()),
        auth_type: Some(transport.key.auth_type.clone()),
        decrypted_api_key: Some(transport.key.decrypted_api_key.clone()),
        decrypted_auth_config: transport.key.decrypted_auth_config.clone(),
        provider_config: transport.provider.config.clone(),
        endpoint_config: transport.endpoint.config.clone(),
        key_config: None,
        network: OAuthNetworkContext::provider_operation(None),
    }
}

pub(crate) fn oauth_error_to_local_refresh_error(
    provider_type: &'static str,
    error: OAuthError,
) -> LocalOAuthRefreshError {
    match error {
        OAuthError::HttpStatus {
            status_code,
            body_excerpt,
        } => LocalOAuthRefreshError::HttpStatus {
            provider_type,
            status_code,
            body_excerpt,
        },
        OAuthError::Transport(message) => LocalOAuthRefreshError::TransportMessage {
            provider_type,
            message,
        },
        OAuthError::InvalidRequest(message)
        | OAuthError::InvalidResponse(message)
        | OAuthError::Storage(message)
        | OAuthError::UnsupportedProvider(message) => LocalOAuthRefreshError::InvalidResponse {
            provider_type,
            message,
        },
        OAuthError::InvalidState => LocalOAuthRefreshError::InvalidResponse {
            provider_type,
            message: "oauth state is invalid or expired".to_string(),
        },
        OAuthError::EncryptionUnavailable => LocalOAuthRefreshError::InvalidResponse {
            provider_type,
            message: "oauth encryption unavailable".to_string(),
        },
    }
}

fn local_refresh_error_to_oauth_error(error: LocalOAuthRefreshError) -> OAuthError {
    match error {
        LocalOAuthRefreshError::Transport { .. } => {
            OAuthError::Transport("oauth refresh request failed".to_string())
        }
        LocalOAuthRefreshError::TransportMessage { message, .. } => OAuthError::Transport(message),
        LocalOAuthRefreshError::HttpStatus {
            status_code,
            body_excerpt,
            ..
        } => OAuthError::HttpStatus {
            status_code,
            body_excerpt,
        },
        LocalOAuthRefreshError::InvalidResponse { message, .. } => {
            OAuthError::InvalidResponse(message)
        }
    }
}

#[async_trait]
pub trait LocalOAuthRefreshAdapter: Send + Sync {
    fn provider_type(&self) -> &'static str;

    fn supports(&self, transport: &GatewayProviderTransportSnapshot) -> bool {
        transport
            .provider
            .provider_type
            .trim()
            .eq_ignore_ascii_case(self.provider_type())
    }

    fn resolve_cached(
        &self,
        transport: &GatewayProviderTransportSnapshot,
        entry: &CachedOAuthEntry,
    ) -> Option<LocalResolvedOAuthRequestAuth>;

    /// Resolves a cache entry that is known to have advanced the caller's
    /// refresh fence. Agent task rotation can safely use the winner even while
    /// the caller still holds the pre-refresh transport snapshot.
    fn resolve_fenced_cached(
        &self,
        transport: &GatewayProviderTransportSnapshot,
        entry: &CachedOAuthEntry,
    ) -> Option<LocalResolvedOAuthRequestAuth> {
        self.resolve_cached(transport, entry)
    }

    /// Resolves the entry returned by this adapter's immediately preceding
    /// refresh. Unlike a reusable cache entry, this entry is expected to have
    /// advanced the transport generation.
    fn resolve_refreshed(
        &self,
        transport: &GatewayProviderTransportSnapshot,
        entry: &CachedOAuthEntry,
    ) -> Option<LocalResolvedOAuthRequestAuth> {
        self.resolve_cached(transport, entry)
    }

    fn resolve_without_refresh(
        &self,
        transport: &GatewayProviderTransportSnapshot,
    ) -> Option<LocalResolvedOAuthRequestAuth>;

    fn should_refresh(
        &self,
        transport: &GatewayProviderTransportSnapshot,
        entry: Option<&CachedOAuthEntry>,
    ) -> bool;

    /// Identifies the credential/configuration generation used by a refresh.
    /// Adapters that support fencing override this method.
    fn refresh_fingerprint(
        &self,
        _transport: &GatewayProviderTransportSnapshot,
        _entry: Option<&CachedOAuthEntry>,
    ) -> Option<String> {
        None
    }

    /// Reconstructs a cache entry from an already-persisted transport after a
    /// distributed refresh waiter reloads the winner.
    fn cached_entry_from_transport(
        &self,
        _transport: &GatewayProviderTransportSnapshot,
    ) -> Option<CachedOAuthEntry> {
        None
    }

    /// Enables bounded negative backoff for transient refresh failures.
    fn should_backoff_after_error(&self, _error: &LocalOAuthRefreshError) -> bool {
        false
    }

    /// Agent task registration is a non-idempotent external mutation and must
    /// not continue unlocked when a configured distributed lock is unavailable.
    fn requires_distributed_refresh_lock(&self) -> bool {
        false
    }

    /// Whether another gateway instance can observe this refresh after the
    /// caller persists its result into the provider transport record.
    fn shares_refresh_through_transport_persistence(&self) -> bool {
        true
    }

    async fn refresh(
        &self,
        executor: &dyn LocalOAuthHttpExecutor,
        transport: &GatewayProviderTransportSnapshot,
        entry: Option<&CachedOAuthEntry>,
    ) -> Result<Option<CachedOAuthEntry>, LocalOAuthRefreshError>;
}

pub struct LocalOAuthRefreshCoordinator {
    adapters: Vec<Arc<dyn LocalOAuthRefreshAdapter>>,
    cache: Mutex<BTreeMap<String, CachedOAuthEntry>>,
    key_locks: Mutex<BTreeMap<String, Arc<Mutex<()>>>>,
    refresh_backoff: Mutex<BTreeMap<String, RefreshBackoffState>>,
}

#[derive(Debug, Clone)]
struct RefreshBackoffState {
    failures: u32,
    retry_after: Instant,
    refresh_fingerprint: Option<String>,
}

impl fmt::Debug for LocalOAuthRefreshCoordinator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalOAuthRefreshCoordinator")
            .field("adapter_count", &self.adapters.len())
            .finish()
    }
}

impl Default for LocalOAuthRefreshCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalOAuthRefreshCoordinator {
    // Keep the lease alive through the 30s upstream HTTP timeout and the
    // subsequent encrypted DB CAS/persistence step. Cancellation still relies
    // on expiry as the last-resort release path.
    const DISTRIBUTED_REFRESH_LOCK_TTL_MS: u64 = 120_000;

    pub fn new() -> Self {
        Self {
            adapters: vec![
                Arc::new(CodexAgentIdentityRefreshAdapter::default()),
                Arc::new(KiroOAuthRefreshAdapter::default()),
                Arc::new(VertexServiceAccountRefreshAdapter),
                Arc::new(GenericOAuthRefreshAdapter::default()),
            ],
            cache: Mutex::new(BTreeMap::new()),
            key_locks: Mutex::new(BTreeMap::new()),
            refresh_backoff: Mutex::new(BTreeMap::new()),
        }
    }

    /// Captures the refresh generation represented by this transport snapshot.
    /// The coordinator cache is intentionally excluded: callers use this value
    /// as the fence for the credential generation that produced their request.
    pub fn refresh_fingerprint_for_transport(
        &self,
        transport: &GatewayProviderTransportSnapshot,
    ) -> Option<String> {
        self.adapters
            .iter()
            .find(|adapter| adapter.supports(transport))
            .and_then(|adapter| adapter.refresh_fingerprint(transport, None))
    }

    async fn lock_for_key(&self, key_id: &str) -> Arc<Mutex<()>> {
        let mut key_locks = self.key_locks.lock().await;
        key_locks
            .entry(key_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    async fn cached_entry(&self, key_id: &str) -> Option<CachedOAuthEntry> {
        self.cache.lock().await.get(key_id).cloned()
    }

    async fn insert_cached_entry(&self, key_id: &str, entry: CachedOAuthEntry) {
        self.cache.lock().await.insert(key_id.to_string(), entry);
    }

    pub async fn store_cached_entry(&self, key_id: &str, entry: CachedOAuthEntry) {
        self.insert_cached_entry(key_id, entry).await;
    }

    pub async fn invalidate_cached_entry(&self, key_id: &str) -> bool {
        let removed = self.cache.lock().await.remove(key_id).is_some();
        self.clear_refresh_backoff(key_id).await;
        removed
    }

    pub async fn resolve_with_result(
        &self,
        executor: &dyn LocalOAuthHttpExecutor,
        transport: &GatewayProviderTransportSnapshot,
        distributed_lock: Option<&RuntimeState>,
        distributed_owner: Option<&str>,
    ) -> Result<Option<LocalOAuthResolution>, LocalOAuthRefreshError> {
        self.resolve_with_result_mode(
            executor,
            transport,
            distributed_lock,
            distributed_owner,
            false,
            None,
        )
        .await
    }

    pub async fn force_refresh_with_result(
        &self,
        executor: &dyn LocalOAuthHttpExecutor,
        transport: &GatewayProviderTransportSnapshot,
        distributed_lock: Option<&RuntimeState>,
        distributed_owner: Option<&str>,
    ) -> Result<Option<LocalOAuthResolution>, LocalOAuthRefreshError> {
        self.force_refresh_with_result_fenced(
            executor,
            transport,
            distributed_lock,
            distributed_owner,
            None,
        )
        .await
    }

    /// Force a refresh unless another request has already advanced the supplied
    /// refresh fence. This prevents a distributed waiter from re-registering a
    /// task after the winner has persisted it.
    pub async fn force_refresh_with_result_fenced(
        &self,
        executor: &dyn LocalOAuthHttpExecutor,
        transport: &GatewayProviderTransportSnapshot,
        distributed_lock: Option<&RuntimeState>,
        distributed_owner: Option<&str>,
        expected_refresh_fingerprint: Option<&str>,
    ) -> Result<Option<LocalOAuthResolution>, LocalOAuthRefreshError> {
        self.resolve_with_result_mode(
            executor,
            transport,
            distributed_lock,
            distributed_owner,
            true,
            expected_refresh_fingerprint,
        )
        .await
    }

    async fn resolve_with_result_mode(
        &self,
        executor: &dyn LocalOAuthHttpExecutor,
        transport: &GatewayProviderTransportSnapshot,
        distributed_lock: Option<&RuntimeState>,
        distributed_owner: Option<&str>,
        force_refresh: bool,
        expected_refresh_fingerprint: Option<&str>,
    ) -> Result<Option<LocalOAuthResolution>, LocalOAuthRefreshError> {
        let Some(adapter) = self
            .adapters
            .iter()
            .find(|adapter| adapter.supports(transport))
        else {
            return Ok(None);
        };
        let key_id = transport.key.id.trim();

        let cached_entry = if key_id.is_empty() {
            None
        } else {
            self.cached_entry(key_id).await
        };
        let shares_refresh_through_transport_persistence =
            adapter.shares_refresh_through_transport_persistence();
        let pre_lock_local_refresh_fingerprint = force_refresh
            .then(|| adapter.refresh_fingerprint(transport, cached_entry.as_ref()))
            .flatten();
        if !force_refresh {
            if let Some(auth) = cached_entry
                .as_ref()
                .and_then(|entry| adapter.resolve_cached(transport, entry))
            {
                return Ok(Some(LocalOAuthResolution::resolved(auth, None)));
            }
            if let Some(auth) = adapter.resolve_without_refresh(transport) {
                return Ok(Some(LocalOAuthResolution::resolved(auth, None)));
            }
            if !adapter.should_refresh(transport, cached_entry.as_ref()) {
                return Ok(None);
            }
        }
        if key_id.is_empty() {
            return Ok(None);
        }

        if force_refresh && shares_refresh_through_transport_persistence {
            if let Some(resolution) = Self::resolve_if_refresh_fence_advanced(
                adapter.as_ref(),
                transport,
                cached_entry.as_ref(),
                expected_refresh_fingerprint,
            ) {
                return Ok(Some(resolution));
            }
        }
        let refresh_fingerprint = adapter.refresh_fingerprint(transport, cached_entry.as_ref());
        if let Some(error) = self
            .backoff_error(
                key_id,
                adapter.provider_type(),
                refresh_fingerprint.as_deref(),
            )
            .await
        {
            return Err(error);
        }

        let key_lock = self.lock_for_key(key_id).await;
        let key_guard = key_lock.lock_owned().await;

        let cached_entry = self.cached_entry(key_id).await;
        if force_refresh {
            let winner_fingerprint = if shares_refresh_through_transport_persistence {
                expected_refresh_fingerprint
            } else {
                pre_lock_local_refresh_fingerprint.as_deref()
            };
            if let Some(resolution) = Self::resolve_if_refresh_fence_advanced(
                adapter.as_ref(),
                transport,
                cached_entry.as_ref(),
                winner_fingerprint,
            ) {
                return Ok(Some(resolution));
            }
        }
        let refresh_fingerprint = adapter.refresh_fingerprint(transport, cached_entry.as_ref());
        if let Some(error) = self
            .backoff_error(
                key_id,
                adapter.provider_type(),
                refresh_fingerprint.as_deref(),
            )
            .await
        {
            return Err(error);
        }
        if !force_refresh {
            if let Some(auth) = cached_entry
                .as_ref()
                .and_then(|entry| adapter.resolve_cached(transport, entry))
            {
                return Ok(Some(LocalOAuthResolution::resolved(auth, None)));
            }
            if let Some(auth) = adapter.resolve_without_refresh(transport) {
                return Ok(Some(LocalOAuthResolution::resolved(auth, None)));
            }
            if !adapter.should_refresh(transport, cached_entry.as_ref()) {
                return Ok(None);
            }
        }

        let distributed_lease = if !shares_refresh_through_transport_persistence {
            None
        } else {
            match (distributed_lock, distributed_owner) {
                (Some(lock), Some(owner)) if !owner.trim().is_empty() => {
                    match lock
                        .lock_try_acquire(
                            &format!("provider_oauth_refresh_lock:{key_id}"),
                            owner,
                            std::time::Duration::from_millis(Self::DISTRIBUTED_REFRESH_LOCK_TTL_MS),
                        )
                        .await
                    {
                        Ok(Some(lease)) => Some(lease),
                        Ok(None) => return Ok(Some(LocalOAuthResolution::refresh_in_flight())),
                        Err(err) => {
                            tracing::warn!(
                                key_id = %key_id,
                                provider_type = adapter.provider_type(),
                                error = ?err,
                                "gateway local oauth refresh distributed lock unavailable"
                            );
                            if adapter.requires_distributed_refresh_lock() {
                                let error = LocalOAuthRefreshError::TransportMessage {
                                    provider_type: adapter.provider_type(),
                                    message: "distributed refresh lock is unavailable".to_string(),
                                };
                                if adapter.should_backoff_after_error(&error) {
                                    self.record_refresh_failure(
                                        key_id,
                                        refresh_fingerprint.as_deref(),
                                    )
                                    .await;
                                }
                                return Err(error);
                            }
                            None
                        }
                    }
                }
                _ => None,
            }
        };

        // Forced refresh still needs the latest rotated refresh_token as input.
        // Otherwise a second overlapping refresh can acquire the lock after the
        // first one completes, then immediately retry with the stale token that
        // came from the original transport snapshot.
        let refresh_entry = cached_entry.as_ref();
        let refresh_result = adapter.refresh(executor, transport, refresh_entry).await;
        let refreshed_entry = match refresh_result {
            Ok(Some(entry)) => {
                self.clear_refresh_backoff(key_id).await;
                entry
            }
            Ok(None) => {
                Self::release_distributed_lease(
                    distributed_lock,
                    distributed_lease.as_ref(),
                    key_id,
                    adapter.provider_type(),
                )
                .await;
                return Ok(None);
            }
            Err(error) => {
                if adapter.should_backoff_after_error(&error) {
                    self.record_refresh_failure(key_id, refresh_fingerprint.as_deref())
                        .await;
                }
                Self::release_distributed_lease(
                    distributed_lock,
                    distributed_lease.as_ref(),
                    key_id,
                    adapter.provider_type(),
                )
                .await;
                return Err(error);
            }
        };
        // Cache publication belongs to the caller after durable persistence.
        // The result still carries the entry so lock-free callers can inspect
        // it or explicitly commit it with `store_cached_entry`.
        let Some(auth) = adapter.resolve_refreshed(transport, &refreshed_entry) else {
            Self::release_distributed_lease(
                distributed_lock,
                distributed_lease.as_ref(),
                key_id,
                adapter.provider_type(),
            )
            .await;
            return Ok(None);
        };
        let local_refresh_guard = if shares_refresh_through_transport_persistence {
            None
        } else {
            Some(LocalOAuthRefreshCommitGuard::new(key_guard))
        };
        Ok(Some(LocalOAuthResolution::refreshed(
            auth,
            refreshed_entry,
            distributed_lease,
            local_refresh_guard,
        )))
    }

    fn resolve_if_refresh_fence_advanced(
        adapter: &dyn LocalOAuthRefreshAdapter,
        transport: &GatewayProviderTransportSnapshot,
        entry: Option<&CachedOAuthEntry>,
        expected_refresh_fingerprint: Option<&str>,
    ) -> Option<LocalOAuthResolution> {
        let expected = expected_refresh_fingerprint?;
        if adapter.refresh_fingerprint(transport, entry).as_deref() == Some(expected) {
            return None;
        }
        entry
            .and_then(|entry| adapter.resolve_fenced_cached(transport, entry))
            .map(|auth| {
                LocalOAuthResolution::reused(
                    auth,
                    entry.expect("cached auth is required when a refresh fence advanced"),
                )
            })
            .or_else(|| {
                adapter
                    .cached_entry_from_transport(transport)
                    .and_then(|entry| {
                        adapter
                            .resolve_cached(transport, &entry)
                            .map(|auth| LocalOAuthResolution::reused(auth, &entry))
                    })
            })
            .or_else(|| {
                adapter
                    .resolve_without_refresh(transport)
                    .map(|auth| LocalOAuthResolution::resolved(auth, None))
            })
    }

    async fn backoff_error(
        &self,
        key_id: &str,
        provider_type: &'static str,
        refresh_fingerprint: Option<&str>,
    ) -> Option<LocalOAuthRefreshError> {
        let mut backoff = self.refresh_backoff.lock().await;
        if backoff
            .get(key_id)
            .is_some_and(|state| state.refresh_fingerprint.as_deref() != refresh_fingerprint)
        {
            backoff.remove(key_id);
            return None;
        }
        let state = backoff.get(key_id)?;
        let remaining = state.retry_after.checked_duration_since(Instant::now())?;
        Some(LocalOAuthRefreshError::InvalidResponse {
            provider_type,
            message: format!(
                "refresh temporarily backed off after {} failed attempts (retry in {}ms)",
                state.failures,
                remaining.as_millis()
            ),
        })
    }

    async fn record_refresh_failure(&self, key_id: &str, refresh_fingerprint: Option<&str>) {
        let mut backoff = self.refresh_backoff.lock().await;
        let state = backoff
            .entry(key_id.to_string())
            .or_insert(RefreshBackoffState {
                failures: 0,
                retry_after: Instant::now(),
                refresh_fingerprint: refresh_fingerprint.map(ToOwned::to_owned),
            });
        if state.refresh_fingerprint.as_deref() != refresh_fingerprint {
            state.failures = 0;
            state.refresh_fingerprint = refresh_fingerprint.map(ToOwned::to_owned);
        }
        state.failures = state.failures.saturating_add(1);
        let exponent = state.failures.saturating_sub(1).min(4);
        let delay = Duration::from_millis(500u64.saturating_mul(1u64 << exponent));
        state.retry_after = Instant::now() + delay.min(Duration::from_secs(8));
    }

    async fn clear_refresh_backoff(&self, key_id: &str) {
        self.refresh_backoff.lock().await.remove(key_id);
    }

    async fn release_distributed_lease(
        distributed_lock: Option<&RuntimeState>,
        lease: Option<&RuntimeLockLease>,
        key_id: &str,
        provider_type: &'static str,
    ) {
        let (Some(lock), Some(lease)) = (distributed_lock, lease) else {
            return;
        };
        if let Err(err) = lock.lock_release(lease).await {
            tracing::warn!(
                key_id = %key_id,
                provider_type,
                error = ?err,
                "gateway local oauth refresh distributed lock release failed"
            );
        }
    }

    pub fn with_adapters_for_tests(adapters: Vec<Arc<dyn LocalOAuthRefreshAdapter>>) -> Self {
        Self {
            adapters,
            cache: Mutex::new(BTreeMap::new()),
            key_locks: Mutex::new(BTreeMap::new()),
            refresh_backoff: Mutex::new(BTreeMap::new()),
        }
    }
}

impl LocalOAuthResolution {
    fn resolved(
        auth: LocalResolvedOAuthRequestAuth,
        refreshed_entry: Option<CachedOAuthEntry>,
    ) -> Self {
        Self {
            auth: Some(auth),
            refreshed_entry,
            refresh_in_flight: false,
            reused_refresh: false,
            distributed_lease: None,
            local_refresh_guard: None,
        }
    }

    fn refreshed(
        auth: LocalResolvedOAuthRequestAuth,
        refreshed_entry: CachedOAuthEntry,
        distributed_lease: Option<RuntimeLockLease>,
        local_refresh_guard: Option<LocalOAuthRefreshCommitGuard>,
    ) -> Self {
        Self {
            auth: Some(auth),
            refreshed_entry: Some(refreshed_entry),
            refresh_in_flight: false,
            reused_refresh: false,
            distributed_lease,
            local_refresh_guard,
        }
    }

    fn reused(auth: LocalResolvedOAuthRequestAuth, entry: &CachedOAuthEntry) -> Self {
        let mut refreshed_entry = entry.clone();
        if let LocalResolvedOAuthRequestAuth::Header { name, value } = &auth {
            refreshed_entry.auth_header_name = name.clone();
            refreshed_entry.auth_header_value = value.clone();
        }
        Self {
            auth: Some(auth),
            refreshed_entry: Some(refreshed_entry),
            refresh_in_flight: false,
            reused_refresh: true,
            distributed_lease: None,
            local_refresh_guard: None,
        }
    }

    fn refresh_in_flight() -> Self {
        Self {
            auth: None,
            refreshed_entry: None,
            refresh_in_flight: true,
            reused_refresh: false,
            distributed_lease: None,
            local_refresh_guard: None,
        }
    }
}

pub fn supports_local_oauth_request_auth_resolution(
    transport: &GatewayProviderTransportSnapshot,
) -> bool {
    is_codex_agent_identity_transport(transport)
        || supports_local_kiro_request_auth_resolution(transport)
        || supports_local_vertex_service_account_auth_resolution(transport)
        || supports_local_generic_oauth_request_auth_resolution(transport)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    use super::super::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };
    use super::{
        validate_local_oauth_request_url, CachedOAuthEntry, LocalOAuthHttpExecutor,
        LocalOAuthRefreshAdapter, LocalOAuthRefreshCoordinator, LocalOAuthRefreshError,
        LocalOAuthResolution, LocalResolvedOAuthRequestAuth, ReqwestLocalOAuthHttpExecutor,
    };
    use async_trait::async_trait;
    use std::sync::Arc;

    #[test]
    fn local_oauth_transport_requires_https_or_literal_loopback_http() {
        for allowed in [
            "https://oauth.example.test/token?tenant=one",
            "http://localhost:8080/token",
            "http://127.42.0.1:8080/token",
            "http://[::1]:8080/token",
        ] {
            assert!(
                validate_local_oauth_request_url("test", allowed).is_ok(),
                "URL should be accepted: {allowed}"
            );
        }
        for rejected in [
            "http://oauth.example.test/token",
            "http://10.0.0.1/token",
            "http://0.0.0.0:8080/token",
            "http://[::ffff:127.0.0.1]:8080/token",
            "https://token@oauth.example.test/token",
            "https://oauth.example.test/token#secret",
            "file:///tmp/token",
        ] {
            assert!(
                validate_local_oauth_request_url("test", rejected).is_err(),
                "URL should be rejected: {rejected}"
            );
        }
    }

    #[test]
    fn local_oauth_url_error_does_not_echo_embedded_credentials() {
        let error = validate_local_oauth_request_url(
            "test",
            "https://sensitive-user:sensitive-password@oauth.example.test/token",
        )
        .expect_err("userinfo should be rejected")
        .to_string();

        assert!(!error.contains("sensitive-user"));
        assert!(!error.contains("sensitive-password"));
    }

    #[test]
    fn local_oauth_debug_output_redacts_request_response_and_cached_credentials() {
        let request = super::LocalOAuthHttpRequest {
            request_id: "provider-oauth:test",
            method: reqwest::Method::POST,
            url: "https://oauth.example.test/token?client_secret=url-secret-canary".to_string(),
            headers: std::collections::BTreeMap::from([
                (
                    "authorization".to_string(),
                    "Bearer request-header-canary".to_string(),
                ),
                ("content-type".to_string(), "application/json".to_string()),
            ]),
            json_body: Some(serde_json::json!({
                "refresh_token": "request-body-canary"
            })),
            body_bytes: None,
        };
        let response = super::LocalOAuthHttpResponse {
            status_code: 401,
            body_text: "{\"access_token\":\"response-body-canary\"}".to_string(),
        };
        let entry = super::CachedOAuthEntry {
            provider_type: "test".to_string(),
            auth_header_name: "authorization".to_string(),
            auth_header_value: "Bearer cached-header-canary".to_string(),
            expires_at_unix_secs: Some(1),
            metadata: Some(serde_json::json!({
                "refresh_token": "cached-metadata-canary"
            })),
            source_fingerprint: Some("non-secret-fingerprint".to_string()),
        };
        let resolution = super::LocalOAuthResolution {
            auth: Some(super::LocalResolvedOAuthRequestAuth::Header {
                name: "authorization".to_string(),
                value: "Bearer resolution-header-canary".to_string(),
            }),
            refreshed_entry: Some(entry.clone()),
            refresh_in_flight: false,
            reused_refresh: false,
            distributed_lease: None,
            local_refresh_guard: None,
        };

        let debug = format!("{request:?} {response:?} {entry:?} {resolution:?}");
        for secret in [
            "url-secret-canary",
            "request-header-canary",
            "request-body-canary",
            "response-body-canary",
            "cached-header-canary",
            "cached-metadata-canary",
            "resolution-header-canary",
        ] {
            assert!(!debug.contains(secret), "debug leaked {secret}");
        }
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn local_oauth_refresh_error_debug_and_display_hide_body_and_transport_details() {
        let http_error = LocalOAuthRefreshError::HttpStatus {
            provider_type: "test",
            status_code: 401,
            body_excerpt: "{\"access_token\":\"refresh-error-canary\"}".to_string(),
        };
        let debug = format!("{http_error:?}");
        let display = http_error.to_string();
        assert!(!debug.contains("refresh-error-canary"));
        assert!(!display.contains("refresh-error-canary"));
        assert_eq!(display, "test oauth refresh returned HTTP 401");
    }

    #[derive(Debug)]
    struct TestAdapter {
        refresh_hits: Arc<AtomicUsize>,
        refresh_with_entry_hits: Arc<AtomicUsize>,
    }

    #[derive(Debug)]
    struct FencedTestAdapter {
        refresh_hits: Arc<AtomicUsize>,
        fail_refresh: Arc<AtomicBool>,
        generation: Arc<AtomicUsize>,
    }

    #[derive(Debug)]
    struct MemoryOnlyFencedTestAdapter {
        refresh_hits: Arc<AtomicUsize>,
        fingerprint_hits: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl LocalOAuthRefreshAdapter for TestAdapter {
        fn provider_type(&self) -> &'static str {
            "test-oauth"
        }

        fn resolve_cached(
            &self,
            _transport: &GatewayProviderTransportSnapshot,
            entry: &CachedOAuthEntry,
        ) -> Option<LocalResolvedOAuthRequestAuth> {
            (entry.provider_type == "test-oauth").then(|| LocalResolvedOAuthRequestAuth::Header {
                name: entry.auth_header_name.clone(),
                value: entry.auth_header_value.clone(),
            })
        }

        fn resolve_without_refresh(
            &self,
            transport: &GatewayProviderTransportSnapshot,
        ) -> Option<LocalResolvedOAuthRequestAuth> {
            let secret = transport.key.decrypted_api_key.trim();
            (!secret.is_empty() && secret != "__placeholder__").then(|| {
                LocalResolvedOAuthRequestAuth::Header {
                    name: "authorization".to_string(),
                    value: format!("Bearer {secret}"),
                }
            })
        }

        fn should_refresh(
            &self,
            transport: &GatewayProviderTransportSnapshot,
            entry: Option<&CachedOAuthEntry>,
        ) -> bool {
            entry.is_none() && transport.key.decrypted_api_key.trim() == "__placeholder__"
        }

        async fn refresh(
            &self,
            _executor: &dyn LocalOAuthHttpExecutor,
            _transport: &GatewayProviderTransportSnapshot,
            entry: Option<&CachedOAuthEntry>,
        ) -> Result<Option<CachedOAuthEntry>, LocalOAuthRefreshError> {
            self.refresh_hits.fetch_add(1, Ordering::SeqCst);
            if entry.is_some() {
                self.refresh_with_entry_hits.fetch_add(1, Ordering::SeqCst);
            }
            Ok(Some(CachedOAuthEntry {
                provider_type: "test-oauth".to_string(),
                auth_header_name: "authorization".to_string(),
                auth_header_value: "Bearer refreshed-token".to_string(),
                expires_at_unix_secs: Some(4_102_444_800),
                metadata: None,
                source_fingerprint: None,
            }))
        }
    }

    #[async_trait]
    impl LocalOAuthRefreshAdapter for FencedTestAdapter {
        fn provider_type(&self) -> &'static str {
            "test-oauth"
        }

        fn resolve_cached(
            &self,
            _transport: &GatewayProviderTransportSnapshot,
            entry: &CachedOAuthEntry,
        ) -> Option<LocalResolvedOAuthRequestAuth> {
            Some(LocalResolvedOAuthRequestAuth::Header {
                name: entry.auth_header_name.clone(),
                value: "fresh-winner-assertion".to_string(),
            })
        }

        fn resolve_without_refresh(
            &self,
            _transport: &GatewayProviderTransportSnapshot,
        ) -> Option<LocalResolvedOAuthRequestAuth> {
            None
        }

        fn should_refresh(
            &self,
            _transport: &GatewayProviderTransportSnapshot,
            _entry: Option<&CachedOAuthEntry>,
        ) -> bool {
            true
        }

        fn refresh_fingerprint(
            &self,
            _transport: &GatewayProviderTransportSnapshot,
            entry: Option<&CachedOAuthEntry>,
        ) -> Option<String> {
            entry
                .and_then(|entry| entry.source_fingerprint.clone())
                .or_else(|| {
                    Some(format!(
                        "generation-{}",
                        self.generation.load(Ordering::SeqCst)
                    ))
                })
        }

        fn should_backoff_after_error(&self, _error: &LocalOAuthRefreshError) -> bool {
            true
        }

        async fn refresh(
            &self,
            _executor: &dyn LocalOAuthHttpExecutor,
            _transport: &GatewayProviderTransportSnapshot,
            _entry: Option<&CachedOAuthEntry>,
        ) -> Result<Option<CachedOAuthEntry>, LocalOAuthRefreshError> {
            self.refresh_hits.fetch_add(1, Ordering::SeqCst);
            if self.fail_refresh.load(Ordering::SeqCst) {
                return Err(LocalOAuthRefreshError::TransportMessage {
                    provider_type: "test-oauth",
                    message: "temporary failure".to_string(),
                });
            }
            Ok(Some(CachedOAuthEntry {
                provider_type: "test-oauth".to_string(),
                auth_header_name: "authorization".to_string(),
                auth_header_value: "stale-winner-cache-value".to_string(),
                expires_at_unix_secs: None,
                metadata: None,
                source_fingerprint: Some(format!(
                    "generation-{}",
                    self.generation.load(Ordering::SeqCst).saturating_add(1)
                )),
            }))
        }
    }

    #[async_trait]
    impl LocalOAuthRefreshAdapter for MemoryOnlyFencedTestAdapter {
        fn provider_type(&self) -> &'static str {
            "test-oauth"
        }

        fn resolve_cached(
            &self,
            _transport: &GatewayProviderTransportSnapshot,
            entry: &CachedOAuthEntry,
        ) -> Option<LocalResolvedOAuthRequestAuth> {
            Some(LocalResolvedOAuthRequestAuth::Header {
                name: entry.auth_header_name.clone(),
                value: entry.auth_header_value.clone(),
            })
        }

        fn resolve_without_refresh(
            &self,
            _transport: &GatewayProviderTransportSnapshot,
        ) -> Option<LocalResolvedOAuthRequestAuth> {
            None
        }

        fn should_refresh(
            &self,
            _transport: &GatewayProviderTransportSnapshot,
            _entry: Option<&CachedOAuthEntry>,
        ) -> bool {
            true
        }

        fn refresh_fingerprint(
            &self,
            _transport: &GatewayProviderTransportSnapshot,
            entry: Option<&CachedOAuthEntry>,
        ) -> Option<String> {
            self.fingerprint_hits.fetch_add(1, Ordering::SeqCst);
            entry
                .and_then(|entry| entry.source_fingerprint.clone())
                .or_else(|| Some("transport-generation".to_string()))
        }

        fn shares_refresh_through_transport_persistence(&self) -> bool {
            false
        }

        async fn refresh(
            &self,
            _executor: &dyn LocalOAuthHttpExecutor,
            _transport: &GatewayProviderTransportSnapshot,
            _entry: Option<&CachedOAuthEntry>,
        ) -> Result<Option<CachedOAuthEntry>, LocalOAuthRefreshError> {
            let hit = self.refresh_hits.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(Some(CachedOAuthEntry {
                provider_type: "test-oauth".to_string(),
                auth_header_name: "authorization".to_string(),
                auth_header_value: format!("Bearer refreshed-token-{hit}"),
                expires_at_unix_secs: Some(4_102_444_800),
                metadata: None,
                source_fingerprint: Some(format!("local-generation-{hit}")),
            }))
        }
    }

    fn sample_transport() -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "test".to_string(),
                provider_type: "test-oauth".to_string(),
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
                api_format: "claude:messages".to_string(),
                api_family: Some("claude".to_string()),
                endpoint_kind: Some("cli".to_string()),
                is_active: true,
                base_url: "https://example.test".to_string(),
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
                auth_type: "bearer".to_string(),
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
                decrypted_auth_config: Some("{\"refresh_token\":\"rt-1\"}".to_string()),
            },
        }
    }

    #[tokio::test]
    async fn coordinator_reuses_runtime_cached_refresh_result() {
        let refresh_hits = Arc::new(AtomicUsize::new(0));
        let refresh_with_entry_hits = Arc::new(AtomicUsize::new(0));
        let coordinator =
            LocalOAuthRefreshCoordinator::with_adapters_for_tests(vec![Arc::new(TestAdapter {
                refresh_hits: Arc::clone(&refresh_hits),
                refresh_with_entry_hits: Arc::clone(&refresh_with_entry_hits),
            })]);
        let transport = sample_transport();
        let executor = ReqwestLocalOAuthHttpExecutor::new(reqwest::Client::new());

        let first = coordinator
            .resolve_with_result(&executor, &transport, None, None)
            .await
            .expect("first resolve should succeed");
        assert!(coordinator
            .cached_entry(transport.key.id.as_str())
            .await
            .is_none());
        coordinator
            .store_cached_entry(
                transport.key.id.as_str(),
                first
                    .as_ref()
                    .and_then(|result| result.refreshed_entry.clone())
                    .expect("first resolve should provide cached entry"),
            )
            .await;
        let second = coordinator
            .resolve_with_result(&executor, &transport, None, None)
            .await
            .expect("second resolve should succeed");

        assert_eq!(refresh_hits.load(Ordering::SeqCst), 1);
        assert_eq!(
            first,
            Some(LocalOAuthResolution {
                auth: Some(LocalResolvedOAuthRequestAuth::Header {
                    name: "authorization".to_string(),
                    value: "Bearer refreshed-token".to_string(),
                }),
                refreshed_entry: Some(CachedOAuthEntry {
                    provider_type: "test-oauth".to_string(),
                    auth_header_name: "authorization".to_string(),
                    auth_header_value: "Bearer refreshed-token".to_string(),
                    expires_at_unix_secs: Some(4_102_444_800),
                    metadata: None,
                    source_fingerprint: None,
                }),
                refresh_in_flight: false,
                reused_refresh: false,
                distributed_lease: None,
                local_refresh_guard: None,
            })
        );
        assert_eq!(
            second,
            Some(LocalOAuthResolution {
                auth: Some(LocalResolvedOAuthRequestAuth::Header {
                    name: "authorization".to_string(),
                    value: "Bearer refreshed-token".to_string(),
                }),
                refreshed_entry: None,
                refresh_in_flight: false,
                reused_refresh: false,
                distributed_lease: None,
                local_refresh_guard: None,
            })
        );
    }

    #[tokio::test]
    async fn coordinator_force_refresh_bypasses_runtime_cache() {
        let refresh_hits = Arc::new(AtomicUsize::new(0));
        let refresh_with_entry_hits = Arc::new(AtomicUsize::new(0));
        let coordinator =
            LocalOAuthRefreshCoordinator::with_adapters_for_tests(vec![Arc::new(TestAdapter {
                refresh_hits: Arc::clone(&refresh_hits),
                refresh_with_entry_hits: Arc::clone(&refresh_with_entry_hits),
            })]);
        let transport = sample_transport();
        let executor = ReqwestLocalOAuthHttpExecutor::new(reqwest::Client::new());

        let first = coordinator
            .resolve_with_result(&executor, &transport, None, None)
            .await
            .expect("initial resolve should succeed");
        coordinator
            .store_cached_entry(
                transport.key.id.as_str(),
                first
                    .as_ref()
                    .and_then(|result| result.refreshed_entry.clone())
                    .expect("first resolve should provide cached entry"),
            )
            .await;
        let forced = coordinator
            .force_refresh_with_result(&executor, &transport, None, None)
            .await
            .expect("forced refresh should succeed");

        assert!(first.and_then(|result| result.refreshed_entry).is_some());
        assert!(forced.and_then(|result| result.refreshed_entry).is_some());
        assert_eq!(refresh_hits.load(Ordering::SeqCst), 2);
        assert_eq!(refresh_with_entry_hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn memory_only_force_refresh_does_not_reuse_preexisting_cache_as_winner() {
        let refresh_hits = Arc::new(AtomicUsize::new(0));
        let fingerprint_hits = Arc::new(AtomicUsize::new(0));
        let coordinator = Arc::new(LocalOAuthRefreshCoordinator::with_adapters_for_tests(vec![
            Arc::new(MemoryOnlyFencedTestAdapter {
                refresh_hits: Arc::clone(&refresh_hits),
                fingerprint_hits: Arc::clone(&fingerprint_hits),
            }),
        ]));
        let transport = sample_transport();
        let executor = ReqwestLocalOAuthHttpExecutor::new(reqwest::Client::new());
        coordinator
            .store_cached_entry(
                transport.key.id.as_str(),
                CachedOAuthEntry {
                    provider_type: "test-oauth".to_string(),
                    auth_header_name: "authorization".to_string(),
                    auth_header_value: "Bearer rejected-token".to_string(),
                    expires_at_unix_secs: Some(4_102_444_800),
                    metadata: None,
                    source_fingerprint: Some("preexisting-local-generation".to_string()),
                },
            )
            .await;

        let mut forced = coordinator
            .force_refresh_with_result_fenced(
                &executor,
                &transport,
                None,
                None,
                Some("transport-generation"),
            )
            .await
            .expect("memory-only force refresh should succeed")
            .expect("memory-only force refresh should resolve");

        assert_eq!(refresh_hits.load(Ordering::SeqCst), 1);
        assert!(!forced.reused_refresh);
        assert!(forced.local_refresh_guard.is_some());
        assert_eq!(
            forced.auth,
            Some(LocalResolvedOAuthRequestAuth::Header {
                name: "authorization".to_string(),
                value: "Bearer refreshed-token-1".to_string(),
            })
        );

        fingerprint_hits.store(0, Ordering::SeqCst);
        let follower_coordinator = Arc::clone(&coordinator);
        let follower_transport = transport.clone();
        let follower = tokio::spawn(async move {
            follower_coordinator
                .force_refresh_with_result_fenced(
                    &ReqwestLocalOAuthHttpExecutor::new(reqwest::Client::new()),
                    &follower_transport,
                    None,
                    None,
                    Some("transport-generation"),
                )
                .await
                .expect("memory-only follower should succeed")
                .expect("memory-only follower should resolve")
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while fingerprint_hits.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("follower should capture its pre-lock fingerprint");

        coordinator
            .store_cached_entry(
                transport.key.id.as_str(),
                forced
                    .refreshed_entry
                    .clone()
                    .expect("leader should provide the memory-only entry"),
            )
            .await;
        forced.local_refresh_guard.take();
        let follower = tokio::time::timeout(Duration::from_secs(1), follower)
            .await
            .expect("follower should unblock after cache publication")
            .expect("follower task should join");

        assert!(follower.reused_refresh);
        assert_eq!(refresh_hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn fenced_force_refresh_reuses_the_winner() {
        let refresh_hits = Arc::new(AtomicUsize::new(0));
        let coordinator = LocalOAuthRefreshCoordinator::with_adapters_for_tests(vec![Arc::new(
            FencedTestAdapter {
                refresh_hits: Arc::clone(&refresh_hits),
                fail_refresh: Arc::new(AtomicBool::new(false)),
                generation: Arc::new(AtomicUsize::new(1)),
            },
        )]);
        let transport = sample_transport();
        let executor = ReqwestLocalOAuthHttpExecutor::new(reqwest::Client::new());

        let first = coordinator
            .force_refresh_with_result_fenced(
                &executor,
                &transport,
                None,
                None,
                Some("generation-1"),
            )
            .await
            .expect("first refresh should succeed")
            .expect("first refresh should resolve");
        coordinator
            .store_cached_entry(
                transport.key.id.as_str(),
                first
                    .refreshed_entry
                    .clone()
                    .expect("first refresh should return an entry to persist"),
            )
            .await;
        let waiter = coordinator
            .force_refresh_with_result_fenced(
                &executor,
                &transport,
                None,
                None,
                Some("generation-1"),
            )
            .await
            .expect("waiter should reuse winner")
            .expect("waiter should resolve");

        assert!(first.refreshed_entry.is_some());
        assert!(waiter.refreshed_entry.is_some());
        assert!(waiter.reused_refresh);
        assert_eq!(
            waiter
                .refreshed_entry
                .as_ref()
                .expect("reused entry")
                .auth_header_value,
            "fresh-winner-assertion"
        );
        assert_eq!(refresh_hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn refresh_failure_enters_bounded_negative_backoff() {
        let refresh_hits = Arc::new(AtomicUsize::new(0));
        let fail_refresh = Arc::new(AtomicBool::new(true));
        let generation = Arc::new(AtomicUsize::new(1));
        let coordinator = LocalOAuthRefreshCoordinator::with_adapters_for_tests(vec![Arc::new(
            FencedTestAdapter {
                refresh_hits: Arc::clone(&refresh_hits),
                fail_refresh: Arc::clone(&fail_refresh),
                generation: Arc::clone(&generation),
            },
        )]);
        let transport = sample_transport();
        let executor = ReqwestLocalOAuthHttpExecutor::new(reqwest::Client::new());

        assert!(coordinator
            .force_refresh_with_result(&executor, &transport, None, None)
            .await
            .is_err());
        let second = coordinator
            .force_refresh_with_result(&executor, &transport, None, None)
            .await
            .expect_err("second refresh should be backed off");
        assert!(second.to_string().contains("temporarily backed off"));
        assert_eq!(refresh_hits.load(Ordering::SeqCst), 1);
        fail_refresh.store(false, Ordering::SeqCst);
        generation.store(2, Ordering::SeqCst);
        assert!(coordinator
            .force_refresh_with_result(&executor, &transport, None, None)
            .await
            .expect("new credential generation should bypass old backoff")
            .is_some());
        assert_eq!(refresh_hits.load(Ordering::SeqCst), 2);
    }
}
