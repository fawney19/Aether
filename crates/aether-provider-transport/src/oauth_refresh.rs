use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use aether_data::redis::{RedisLockKey, RedisLockRunner};
use aether_oauth::core::OAuthError;
use aether_oauth::network::{
    OAuthHttpExecutor, OAuthHttpRequest, OAuthHttpResponse, OAuthNetworkContext,
};
use aether_oauth::provider::ProviderOAuthTransportContext;
use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;
use tokio::sync::Mutex;

use super::generic_oauth::supports_local_generic_oauth_request_auth_resolution;
pub use super::generic_oauth::GenericOAuthRefreshAdapter;
use super::kiro::{
    supports_local_kiro_request_auth_resolution, KiroOAuthRefreshAdapter, KiroRequestAuth,
};
use super::snapshot::GatewayProviderTransportSnapshot;

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum LocalResolvedOAuthRequestAuth {
    #[allow(dead_code)]
    Header {
        name: String,
        value: String,
    },
    Kiro(KiroRequestAuth),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalOAuthResolution {
    pub auth: Option<LocalResolvedOAuthRequestAuth>,
    pub refreshed_entry: Option<CachedOAuthEntry>,
    pub refresh_in_flight: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CachedOAuthEntry {
    pub provider_type: String,
    pub auth_header_name: String,
    pub auth_header_value: String,
    pub expires_at_unix_secs: Option<u64>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalOAuthHttpRequest {
    pub request_id: &'static str,
    pub method: reqwest::Method,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub json_body: Option<Value>,
    pub body_bytes: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalOAuthHttpResponse {
    pub status_code: u16,
    pub body_text: String,
}

#[derive(Debug, Error)]
pub enum LocalOAuthRefreshError {
    #[error("{provider_type} oauth refresh request failed: {source}")]
    Transport {
        provider_type: &'static str,
        #[source]
        source: reqwest::Error,
    },
    #[error("{provider_type} oauth refresh returned HTTP {status_code}: {body_excerpt}")]
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

#[async_trait]
impl LocalOAuthHttpExecutor for ReqwestLocalOAuthHttpExecutor {
    async fn execute(
        &self,
        provider_type: &'static str,
        _transport: &GatewayProviderTransportSnapshot,
        request: &LocalOAuthHttpRequest,
    ) -> Result<LocalOAuthHttpResponse, LocalOAuthRefreshError> {
        let mut builder = self
            .client
            .request(request.method.clone(), request.url.as_str());
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        if let Some(json_body) = request.json_body.as_ref() {
            builder = builder.json(json_body);
        } else if let Some(body_bytes) = request.body_bytes.as_ref() {
            builder = builder.body(body_bytes.clone());
        }

        let response =
            builder
                .send()
                .await
                .map_err(|source| LocalOAuthRefreshError::Transport {
                    provider_type,
                    source,
                })?;
        let status_code = response.status().as_u16();
        let body_text =
            response
                .text()
                .await
                .map_err(|source| LocalOAuthRefreshError::Transport {
                    provider_type,
                    source,
                })?;
        Ok(LocalOAuthHttpResponse {
            status_code,
            body_text,
        })
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
        LocalOAuthRefreshError::Transport { source, .. } => {
            OAuthError::Transport(source.to_string())
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

    fn resolve_without_refresh(
        &self,
        transport: &GatewayProviderTransportSnapshot,
    ) -> Option<LocalResolvedOAuthRequestAuth>;

    fn should_refresh(
        &self,
        transport: &GatewayProviderTransportSnapshot,
        entry: Option<&CachedOAuthEntry>,
    ) -> bool;

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
    const DISTRIBUTED_REFRESH_LOCK_TTL_MS: u64 = 30_000;

    pub fn new() -> Self {
        Self {
            adapters: vec![
                Arc::new(KiroOAuthRefreshAdapter::default()),
                Arc::new(GenericOAuthRefreshAdapter::default()),
            ],
            cache: Mutex::new(BTreeMap::new()),
            key_locks: Mutex::new(BTreeMap::new()),
        }
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
        self.cache.lock().await.remove(key_id).is_some()
    }

    pub async fn resolve_with_result(
        &self,
        executor: &dyn LocalOAuthHttpExecutor,
        transport: &GatewayProviderTransportSnapshot,
        distributed_lock: Option<&RedisLockRunner>,
        distributed_owner: Option<&str>,
    ) -> Result<Option<LocalOAuthResolution>, LocalOAuthRefreshError> {
        self.resolve_with_result_mode(
            executor,
            transport,
            distributed_lock,
            distributed_owner,
            false,
        )
        .await
    }

    pub async fn force_refresh_with_result(
        &self,
        executor: &dyn LocalOAuthHttpExecutor,
        transport: &GatewayProviderTransportSnapshot,
        distributed_lock: Option<&RedisLockRunner>,
        distributed_owner: Option<&str>,
    ) -> Result<Option<LocalOAuthResolution>, LocalOAuthRefreshError> {
        self.resolve_with_result_mode(
            executor,
            transport,
            distributed_lock,
            distributed_owner,
            true,
        )
        .await
    }

    async fn resolve_with_result_mode(
        &self,
        executor: &dyn LocalOAuthHttpExecutor,
        transport: &GatewayProviderTransportSnapshot,
        distributed_lock: Option<&RedisLockRunner>,
        distributed_owner: Option<&str>,
        force_refresh: bool,
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

        let key_lock = self.lock_for_key(key_id).await;
        let _key_guard = key_lock.lock().await;

        let cached_entry = self.cached_entry(key_id).await;
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

        let distributed_lease = match (distributed_lock, distributed_owner) {
            (Some(lock), Some(owner)) if !owner.trim().is_empty() => {
                let lock_key = RedisLockKey(format!("provider_oauth_refresh_lock:{key_id}"));
                match lock
                    .try_acquire(
                        &lock_key,
                        owner,
                        Some(Self::DISTRIBUTED_REFRESH_LOCK_TTL_MS),
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
                        None
                    }
                }
            }
            _ => None,
        };

        // Forced refresh still needs the latest rotated refresh_token as input.
        // Otherwise a second overlapping refresh can acquire the lock after the
        // first one completes, then immediately retry with the stale token that
        // came from the original transport snapshot.
        let refresh_entry = cached_entry.as_ref();
        let refresh_result = adapter.refresh(executor, transport, refresh_entry).await;
        if let (Some(lock), Some(lease)) = (distributed_lock, distributed_lease.as_ref()) {
            if let Err(err) = lock.release(lease).await {
                tracing::warn!(
                    key_id = %key_id,
                    provider_type = adapter.provider_type(),
                    error = ?err,
                    "gateway local oauth refresh distributed lock release failed"
                );
            }
        }
        let Some(refreshed_entry) = refresh_result? else {
            return Ok(None);
        };
        Ok(adapter
            .resolve_cached(transport, &refreshed_entry)
            .map(|auth| LocalOAuthResolution::resolved(auth, Some(refreshed_entry))))
    }

    pub fn with_adapters_for_tests(adapters: Vec<Arc<dyn LocalOAuthRefreshAdapter>>) -> Self {
        Self {
            adapters,
            cache: Mutex::new(BTreeMap::new()),
            key_locks: Mutex::new(BTreeMap::new()),
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
        }
    }

    fn refresh_in_flight() -> Self {
        Self {
            auth: None,
            refreshed_entry: None,
            refresh_in_flight: true,
        }
    }
}

pub fn supports_local_oauth_request_auth_resolution(
    transport: &GatewayProviderTransportSnapshot,
) -> bool {
    supports_local_kiro_request_auth_resolution(transport)
        || supports_local_generic_oauth_request_auth_resolution(transport)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::super::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };
    use super::{
        CachedOAuthEntry, LocalOAuthHttpExecutor, LocalOAuthRefreshAdapter,
        LocalOAuthRefreshCoordinator, LocalOAuthRefreshError, LocalOAuthResolution,
        LocalResolvedOAuthRequestAuth, ReqwestLocalOAuthHttpExecutor,
    };
    use async_trait::async_trait;
    use std::sync::Arc;

    #[derive(Debug)]
    struct TestAdapter {
        refresh_hits: Arc<AtomicUsize>,
        refresh_with_entry_hits: Arc<AtomicUsize>,
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
        coordinator
            .insert_cached_entry(
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
                }),
                refresh_in_flight: false,
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
            .insert_cached_entry(
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
}
