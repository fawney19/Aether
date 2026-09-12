use std::collections::BTreeMap;

use aether_oauth::provider::providers::{
    GenericProviderOAuthAdapter, GENERIC_PROVIDER_OAUTH_TEMPLATES,
};
use aether_oauth::provider::{ProviderOAuthAccount, ProviderOAuthAdapter, ProviderOAuthTokenSet};
use async_trait::async_trait;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::oauth_refresh::{
    oauth_error_to_local_refresh_error, provider_oauth_transport_context_from_snapshot,
    CachedOAuthEntry, LocalOAuthHttpExecutor, LocalOAuthRefreshAdapter, LocalOAuthRefreshError,
    LocalResolvedOAuthRequestAuth, ProviderOAuthLocalHttpExecutor,
};
use super::snapshot::GatewayProviderTransportSnapshot;

const AUTH_HEADER_NAME: &str = "authorization";
const OAUTH_REFRESH_SKEW_SECS: u64 = 120;
const PLACEHOLDER_API_KEY: &str = "__placeholder__";

pub fn supports_local_generic_oauth_request_auth_resolution(
    transport: &GatewayProviderTransportSnapshot,
) -> bool {
    transport.key.auth_type.trim().eq_ignore_ascii_case("oauth")
        && generic_provider_type(transport.provider.provider_type.as_str()).is_some()
}

pub fn resolve_local_generic_oauth_transport_authorization(
    transport: &GatewayProviderTransportSnapshot,
) -> Option<String> {
    if !supports_local_generic_oauth_request_auth_resolution(transport) {
        return None;
    }
    if let Some(value) =
        auth_config_authorization_header(transport.key.decrypted_auth_config.as_deref())
    {
        return if bearer_access_token(&value).is_some() {
            Some(value)
        } else {
            None
        };
    }

    let auth_config = GenericOAuthRefreshAdapter::auth_config_from_transport(transport);
    let refreshable = auth_config
        .as_ref()
        .and_then(refresh_token_from_auth_config)
        .is_some();
    if refreshable && auth_config_expires_soon(auth_config.as_ref()) {
        return None;
    }

    let secret = transport.key.decrypted_api_key.trim();
    if !secret.is_empty() && secret != PLACEHOLDER_API_KEY {
        return Some(format!("Bearer {secret}"));
    }

    auth_config
        .as_ref()
        .and_then(access_token_from_auth_config)
        .map(|token| format!("Bearer {token}"))
}

#[derive(Clone, Default)]
pub struct GenericOAuthRefreshAdapter {
    token_url_overrides: BTreeMap<String, String>,
    oauth_credentials_overrides: BTreeMap<String, (String, String)>,
}

impl std::fmt::Debug for GenericOAuthRefreshAdapter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GenericOAuthRefreshAdapter")
            .field(
                "token_url_override_provider_types",
                &self.token_url_overrides.keys().collect::<Vec<_>>(),
            )
            .field(
                "oauth_credentials_override_provider_types",
                &self.oauth_credentials_overrides.keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl GenericOAuthRefreshAdapter {
    pub fn with_token_url_for_tests(
        mut self,
        provider_type: &str,
        token_url: impl Into<String>,
    ) -> Self {
        self.token_url_overrides
            .insert(provider_type.trim().to_ascii_lowercase(), token_url.into());
        self
    }

    pub fn with_oauth_credentials_for_tests(
        mut self,
        provider_type: &str,
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
    ) -> Self {
        self.oauth_credentials_overrides.insert(
            provider_type.trim().to_ascii_lowercase(),
            (client_id.into(), client_secret.into()),
        );
        self
    }

    fn adapter_for_provider_type(
        &self,
        provider_type: &'static str,
    ) -> Option<GenericProviderOAuthAdapter> {
        let mut adapter = GenericProviderOAuthAdapter::for_provider_type(provider_type)?;
        if let Some((client_id, client_secret)) =
            self.oauth_credentials_overrides.get(provider_type)
        {
            adapter = adapter.with_oauth_credentials_for_tests(client_id, client_secret);
        }
        if let Some(token_url) = self.token_url_overrides.get(provider_type) {
            return Some(adapter.with_token_url_override(token_url.clone()));
        }
        Some(adapter)
    }

    fn auth_config_from_transport(transport: &GatewayProviderTransportSnapshot) -> Option<Value> {
        transport
            .key
            .decrypted_auth_config
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(|value| serde_json::from_str::<Value>(value).ok())
    }

    fn auth_config_from_entry(
        transport: &GatewayProviderTransportSnapshot,
        entry: &CachedOAuthEntry,
    ) -> Option<Value> {
        entry
            .metadata
            .as_ref()
            .filter(|_| {
                entry
                    .provider_type
                    .eq_ignore_ascii_case(transport.provider.provider_type.as_str())
                    && generic_oauth_cached_entry_matches_transport(transport, entry)
            })
            .cloned()
    }

    fn auth_config_updated_at(auth_config: &Value) -> Option<u64> {
        auth_config
            .as_object()
            .and_then(|object| object.get("updated_at"))
            .and_then(|value| parse_u64_value(Some(value)))
    }

    fn base_auth_config(
        &self,
        transport: &GatewayProviderTransportSnapshot,
        entry: Option<&CachedOAuthEntry>,
    ) -> Option<Value> {
        let cached = entry.and_then(|cached| Self::auth_config_from_entry(transport, cached));
        let transport_auth = Self::auth_config_from_transport(transport);

        match (cached, transport_auth) {
            (Some(cached), Some(transport_auth)) => {
                let cached_updated_at = Self::auth_config_updated_at(&cached);
                let transport_updated_at = Self::auth_config_updated_at(&transport_auth);
                if transport_updated_at > cached_updated_at {
                    Some(transport_auth)
                } else {
                    Some(cached)
                }
            }
            (Some(cached), None) => Some(cached),
            (None, Some(transport_auth)) => Some(transport_auth),
            (None, None) => None,
        }
    }

    fn resolve_direct_header(
        &self,
        transport: &GatewayProviderTransportSnapshot,
    ) -> Option<LocalResolvedOAuthRequestAuth> {
        Some(LocalResolvedOAuthRequestAuth::Header {
            name: AUTH_HEADER_NAME.to_string(),
            value: resolve_local_generic_oauth_transport_authorization(transport)?,
        })
    }

    fn build_cached_entry(
        provider_type: &'static str,
        transport: &GatewayProviderTransportSnapshot,
        mut refreshed: ProviderOAuthTokenSet,
    ) -> CachedOAuthEntry {
        let auth_header_value = refreshed.token_set.bearer_header_value();
        synchronize_authorization_overrides(&mut refreshed.auth_config, &auth_header_value);
        CachedOAuthEntry {
            provider_type: provider_type.to_string(),
            auth_header_name: AUTH_HEADER_NAME.to_string(),
            auth_header_value,
            expires_at_unix_secs: refreshed.token_set.expires_at_unix_secs,
            metadata: Some(refreshed.auth_config),
            source_fingerprint: Some(generic_oauth_transport_source_fingerprint(transport)),
        }
    }
}

#[async_trait]
impl LocalOAuthRefreshAdapter for GenericOAuthRefreshAdapter {
    fn provider_type(&self) -> &'static str {
        "generic_oauth"
    }

    fn supports(&self, transport: &GatewayProviderTransportSnapshot) -> bool {
        supports_local_generic_oauth_request_auth_resolution(transport)
    }

    fn resolve_cached(
        &self,
        transport: &GatewayProviderTransportSnapshot,
        entry: &CachedOAuthEntry,
    ) -> Option<LocalResolvedOAuthRequestAuth> {
        if !entry
            .provider_type
            .eq_ignore_ascii_case(transport.provider.provider_type.as_str())
        {
            return None;
        }
        if auth_config_authorization_header(transport.key.decrypted_auth_config.as_deref())
            .is_some()
        {
            return resolve_local_generic_oauth_transport_authorization(transport).map(|value| {
                LocalResolvedOAuthRequestAuth::Header {
                    name: AUTH_HEADER_NAME.to_string(),
                    value,
                }
            });
        }
        if !generic_oauth_cached_entry_matches_transport(transport, entry) {
            return None;
        }
        if expires_at_requires_refresh(entry.expires_at_unix_secs) {
            return None;
        }

        let name = entry.auth_header_name.trim();
        let value = entry.auth_header_value.trim();
        if name.is_empty() || value.is_empty() {
            return None;
        }

        Some(LocalResolvedOAuthRequestAuth::Header {
            name: name.to_ascii_lowercase(),
            value: value.to_string(),
        })
    }

    fn resolve_fenced_cached(
        &self,
        transport: &GatewayProviderTransportSnapshot,
        entry: &CachedOAuthEntry,
    ) -> Option<LocalResolvedOAuthRequestAuth> {
        generic_oauth_successor_entry_matches_transport(transport, entry)
            .then(|| resolved_entry_header(entry))
            .flatten()
    }

    fn resolve_refreshed(
        &self,
        transport: &GatewayProviderTransportSnapshot,
        entry: &CachedOAuthEntry,
    ) -> Option<LocalResolvedOAuthRequestAuth> {
        generic_oauth_entry_belongs_to_transport(transport, entry)
            .then(|| resolved_entry_header(entry))
            .flatten()
    }

    fn resolve_without_refresh(
        &self,
        transport: &GatewayProviderTransportSnapshot,
    ) -> Option<LocalResolvedOAuthRequestAuth> {
        self.resolve_direct_header(transport)
    }

    fn should_refresh(
        &self,
        transport: &GatewayProviderTransportSnapshot,
        entry: Option<&CachedOAuthEntry>,
    ) -> bool {
        if !supports_local_generic_oauth_request_auth_resolution(transport) {
            return false;
        }
        if entry
            .and_then(|cached| self.resolve_cached(transport, cached))
            .is_some()
            || self.resolve_direct_header(transport).is_some()
        {
            return false;
        }

        self.base_auth_config(transport, entry)
            .as_ref()
            .and_then(refresh_token_from_auth_config)
            .is_some()
    }

    fn refresh_fingerprint(
        &self,
        transport: &GatewayProviderTransportSnapshot,
        entry: Option<&CachedOAuthEntry>,
    ) -> Option<String> {
        generic_oauth_refresh_fingerprint(transport, entry)
    }

    fn cached_entry_from_transport(
        &self,
        transport: &GatewayProviderTransportSnapshot,
    ) -> Option<CachedOAuthEntry> {
        let provider_type = generic_provider_type(transport.provider.provider_type.as_str())?;
        let LocalResolvedOAuthRequestAuth::Header { name, value } =
            self.resolve_direct_header(transport)?
        else {
            return None;
        };
        let metadata = Self::auth_config_from_transport(transport);
        let expires_at_unix_secs = transport
            .key
            .expires_at_unix_secs
            .or_else(|| metadata.as_ref().and_then(auth_config_expires_at));
        Some(CachedOAuthEntry {
            provider_type: provider_type.to_string(),
            auth_header_name: name,
            auth_header_value: value,
            expires_at_unix_secs,
            metadata,
            source_fingerprint: Some(generic_oauth_transport_source_fingerprint(transport)),
        })
    }

    async fn refresh(
        &self,
        executor: &dyn LocalOAuthHttpExecutor,
        transport: &GatewayProviderTransportSnapshot,
        entry: Option<&CachedOAuthEntry>,
    ) -> Result<Option<CachedOAuthEntry>, LocalOAuthRefreshError> {
        let Some(provider_type) = generic_provider_type(transport.provider.provider_type.as_str())
        else {
            return Ok(None);
        };
        let Some(auth_config) = self.base_auth_config(transport, entry) else {
            return Ok(None);
        };
        let Some(refresh_token) = refresh_token_from_auth_config(&auth_config) else {
            tracing::warn!(
                key_id = %transport.key.id,
                provider_id = %transport.provider.id,
                provider_type,
                "gateway generic oauth refresh skipped because auth_config has no refresh_token"
            );
            return Ok(None);
        };
        let Some(adapter) = self.adapter_for_provider_type(provider_type) else {
            return Ok(None);
        };

        tracing::info!(
            key_id = %transport.key.id,
            provider_id = %transport.provider.id,
            endpoint_id = %transport.endpoint.id,
            provider_type,
            request_refresh_token_len = refresh_token.len(),
            "gateway generic oauth refresh delegated to provider oauth adapter"
        );

        let oauth_executor =
            ProviderOAuthLocalHttpExecutor::new(provider_type, transport, executor);
        let ctx = provider_oauth_transport_context_from_snapshot(transport);
        let account = ProviderOAuthAccount {
            provider_type: provider_type.to_string(),
            access_token: current_access_token(transport, entry).unwrap_or_default(),
            expires_at_unix_secs: auth_config_expires_at(&auth_config),
            auth_config,
            identity: BTreeMap::new(),
        };
        let refreshed = adapter
            .refresh(&oauth_executor, &ctx, &account)
            .await
            .map_err(|error| oauth_error_to_local_refresh_error(provider_type, error))?;

        tracing::info!(
            key_id = %transport.key.id,
            provider_id = %transport.provider.id,
            endpoint_id = %transport.endpoint.id,
            provider_type,
            expires_at_unix_secs = ?refreshed.token_set.expires_at_unix_secs,
            response_has_refresh_token = refreshed.token_set.refresh_token.is_some(),
            "gateway generic oauth refresh succeeded"
        );

        Ok(Some(Self::build_cached_entry(
            provider_type,
            transport,
            refreshed,
        )))
    }
}

fn generic_oauth_transport_source_fingerprint(
    transport: &GatewayProviderTransportSnapshot,
) -> String {
    let auth_config = transport
        .key
        .decrypted_auth_config
        .as_deref()
        .unwrap_or_default();
    generic_oauth_credential_fingerprint(
        transport.provider.provider_type.as_str(),
        transport.key.auth_type.as_str(),
        auth_config,
        transport.key.decrypted_api_key.as_str(),
    )
}

fn generic_oauth_credential_fingerprint(
    provider_type: &str,
    auth_type: &str,
    auth_config: &str,
    access_token: &str,
) -> String {
    let provider_type = provider_type.trim().to_ascii_lowercase();
    let auth_type = auth_type.trim().to_ascii_lowercase();
    let mut digest = Sha256::new();
    for field in [
        provider_type.as_bytes(),
        auth_type.as_bytes(),
        auth_config.as_bytes(),
        access_token.as_bytes(),
    ] {
        digest.update((field.len() as u64).to_be_bytes());
        digest.update(field);
    }
    format!("{:x}", digest.finalize())
}

fn generic_oauth_cached_entry_matches_transport(
    transport: &GatewayProviderTransportSnapshot,
    entry: &CachedOAuthEntry,
) -> bool {
    let transport_fingerprint = generic_oauth_transport_source_fingerprint(transport);
    entry.source_fingerprint.as_deref() == Some(transport_fingerprint.as_str())
}

fn generic_oauth_entry_belongs_to_transport(
    transport: &GatewayProviderTransportSnapshot,
    entry: &CachedOAuthEntry,
) -> bool {
    entry
        .provider_type
        .eq_ignore_ascii_case(transport.provider.provider_type.as_str())
        && generic_oauth_cached_entry_matches_transport(transport, entry)
}

fn generic_oauth_successor_entry_matches_transport(
    transport: &GatewayProviderTransportSnapshot,
    entry: &CachedOAuthEntry,
) -> bool {
    generic_oauth_entry_belongs_to_transport(transport, entry)
        && !expires_at_requires_refresh(entry.expires_at_unix_secs)
        && resolved_entry_header(entry).is_some()
        && entry.metadata.is_some()
}

fn generic_oauth_refresh_fingerprint(
    transport: &GatewayProviderTransportSnapshot,
    entry: Option<&CachedOAuthEntry>,
) -> Option<String> {
    if !supports_local_generic_oauth_request_auth_resolution(transport) {
        return None;
    }
    entry
        .filter(|entry| generic_oauth_successor_entry_matches_transport(transport, entry))
        .and_then(|entry| {
            let metadata = serde_json::to_string(entry.metadata.as_ref()?).ok()?;
            let access_token = bearer_access_token(entry.auth_header_value.as_str())?;
            Some(generic_oauth_credential_fingerprint(
                transport.provider.provider_type.as_str(),
                transport.key.auth_type.as_str(),
                metadata.as_str(),
                access_token,
            ))
        })
        .or_else(|| Some(generic_oauth_transport_source_fingerprint(transport)))
}

fn resolved_entry_header(entry: &CachedOAuthEntry) -> Option<LocalResolvedOAuthRequestAuth> {
    let name = entry.auth_header_name.trim();
    let value = entry.auth_header_value.trim();
    if name.is_empty() || value.is_empty() {
        return None;
    }
    Some(LocalResolvedOAuthRequestAuth::Header {
        name: name.to_ascii_lowercase(),
        value: value.to_string(),
    })
}

fn bearer_access_token(authorization: &str) -> Option<&str> {
    let mut parts = authorization.split_ascii_whitespace();
    let scheme = parts.next()?;
    let token = parts.next()?;
    (scheme.eq_ignore_ascii_case("bearer") && parts.next().is_none()).then_some(token)
}

fn synchronize_authorization_overrides(auth_config: &mut Value, authorization: &str) {
    let Some(object) = auth_config.as_object_mut() else {
        return;
    };
    for (key, value) in object.iter_mut() {
        match key.trim().to_ascii_lowercase().as_str() {
            "headers" | "extra_headers" | "extraheaders" => {
                let Some(headers) = value.as_object_mut() else {
                    continue;
                };
                for (header_name, header_value) in headers.iter_mut() {
                    if header_name.trim().eq_ignore_ascii_case(AUTH_HEADER_NAME) {
                        *header_value = Value::String(authorization.to_string());
                    }
                }
            }
            "transport" | "request" => {
                synchronize_authorization_overrides(value, authorization);
            }
            _ => {}
        }
    }
}

fn generic_provider_type(provider_type: &str) -> Option<&'static str> {
    let normalized = provider_type.trim();
    GENERIC_PROVIDER_OAUTH_TEMPLATES
        .iter()
        .find(|template| normalized.eq_ignore_ascii_case(template.provider_type))
        .map(|template| template.provider_type)
}

fn refresh_token_from_auth_config(auth_config: &Value) -> Option<String> {
    let object = auth_config.as_object()?;
    ["refresh_token", "refreshToken"]
        .iter()
        .find_map(|field| object.get(*field).and_then(non_empty_string))
}

fn access_token_from_auth_config(auth_config: &Value) -> Option<String> {
    let object = auth_config.as_object()?;
    ["access_token", "accessToken"]
        .iter()
        .find_map(|field| object.get(*field).and_then(non_empty_string))
}

fn auth_config_expires_at(auth_config: &Value) -> Option<u64> {
    auth_config
        .as_object()
        .and_then(|object| object.get("expires_at"))
        .and_then(|value| parse_u64_value(Some(value)))
}

fn auth_config_expires_soon(auth_config: Option<&Value>) -> bool {
    expires_at_requires_refresh(auth_config.and_then(auth_config_expires_at))
}

fn expires_at_requires_refresh(expires_at_unix_secs: Option<u64>) -> bool {
    expires_at_unix_secs
        .map(|expires_at_unix_secs| {
            aether_oauth::core::current_unix_secs()
                >= expires_at_unix_secs.saturating_sub(OAUTH_REFRESH_SKEW_SECS)
        })
        .unwrap_or(false)
}

fn parse_u64_value(value: Option<&Value>) -> Option<u64> {
    match value? {
        Value::Number(number) => number.as_u64(),
        Value::String(string) => string.trim().parse::<u64>().ok(),
        _ => None,
    }
}

fn non_empty_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn auth_config_authorization_header(raw_auth_config: Option<&str>) -> Option<String> {
    let mut headers = BTreeMap::new();
    crate::auth_config::apply_local_auth_config_header_overrides(&mut headers, raw_auth_config);
    headers
        .remove(AUTH_HEADER_NAME)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn current_access_token(
    transport: &GatewayProviderTransportSnapshot,
    entry: Option<&CachedOAuthEntry>,
) -> Option<String> {
    entry
        .filter(|entry| generic_oauth_cached_entry_matches_transport(transport, entry))
        .and_then(|entry| {
            entry
                .auth_header_value
                .trim()
                .strip_prefix("Bearer ")
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
        .or_else(|| {
            let secret = transport.key.decrypted_api_key.trim();
            (!secret.is_empty() && secret != PLACEHOLDER_API_KEY).then(|| secret.to_string())
        })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;
    use serde_json::json;

    use super::super::oauth_refresh::{
        CachedOAuthEntry, LocalOAuthHttpExecutor, LocalOAuthHttpRequest, LocalOAuthHttpResponse,
        LocalOAuthRefreshAdapter, LocalOAuthRefreshCoordinator, LocalOAuthRefreshError,
        LocalResolvedOAuthRequestAuth,
    };
    use super::super::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };
    use super::{
        current_access_token, generic_oauth_transport_source_fingerprint,
        resolve_local_generic_oauth_transport_authorization, GenericOAuthRefreshAdapter,
    };

    #[derive(Debug)]
    struct StaticTokenExecutor {
        hits: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl LocalOAuthHttpExecutor for StaticTokenExecutor {
        async fn execute(
            &self,
            _provider_type: &'static str,
            _transport: &GatewayProviderTransportSnapshot,
            _request: &LocalOAuthHttpRequest,
        ) -> Result<LocalOAuthHttpResponse, LocalOAuthRefreshError> {
            self.hits.fetch_add(1, Ordering::SeqCst);
            Ok(LocalOAuthHttpResponse {
                status_code: 200,
                body_text: json!({
                    "access_token": "fresh-access-token",
                    "expires_in": 3600,
                    "token_type": "Bearer"
                })
                .to_string(),
            })
        }
    }

    fn sample_transport() -> GatewayProviderTransportSnapshot {
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
                endpoint_kind: Some("responses".to_string()),
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
                name: "OAuth headers".to_string(),
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
                decrypted_auth_config: Some(
                    json!({
                        "provider_type": "codex",
                        "access_token_import_temporary": true,
                        "headers": {
                            "Authorization": "Bearer imported-session",
                            "Host": "blocked.example"
                        }
                    })
                    .to_string(),
                ),
            },
        }
    }

    #[test]
    fn resolves_imported_authorization_header_without_api_key_secret() {
        let adapter = GenericOAuthRefreshAdapter::default();
        let auth = adapter
            .resolve_without_refresh(&sample_transport())
            .expect("auth_config authorization header should resolve");

        assert_eq!(
            auth,
            LocalResolvedOAuthRequestAuth::Header {
                name: "authorization".to_string(),
                value: "Bearer imported-session".to_string(),
            }
        );
    }

    #[test]
    fn transport_authorization_uses_one_effective_bearer_generation() {
        let mut transport = sample_transport();
        transport.key.decrypted_api_key = "api-access-token".to_string();
        transport.key.decrypted_auth_config = Some(
            json!({
                "accessToken": "legacy-access-token",
                "request": {
                    "extraHeaders": {
                        "Authorization": "Bearer nested-override-token"
                    }
                }
            })
            .to_string(),
        );
        assert_eq!(
            resolve_local_generic_oauth_transport_authorization(&transport).as_deref(),
            Some("Bearer nested-override-token")
        );

        transport.key.decrypted_auth_config =
            Some(json!({"accessToken": "legacy-access-token"}).to_string());
        assert_eq!(
            resolve_local_generic_oauth_transport_authorization(&transport).as_deref(),
            Some("Bearer api-access-token")
        );

        transport.key.decrypted_api_key = "__placeholder__".to_string();
        assert_eq!(
            resolve_local_generic_oauth_transport_authorization(&transport).as_deref(),
            Some("Bearer legacy-access-token")
        );
    }

    #[test]
    fn non_bearer_authorization_override_does_not_fall_back_to_stale_token() {
        let mut transport = sample_transport();
        transport.key.decrypted_api_key = "api-access-token".to_string();
        transport.key.decrypted_auth_config = Some(
            json!({
                "access_token": "legacy-access-token",
                "headers": {"Authorization": "Basic imported-session"}
            })
            .to_string(),
        );

        assert!(resolve_local_generic_oauth_transport_authorization(&transport).is_none());
        assert!(GenericOAuthRefreshAdapter::default()
            .resolve_without_refresh(&transport)
            .is_none());
    }

    #[test]
    fn auth_config_authorization_header_overrides_cached_oauth_entry() {
        let adapter = GenericOAuthRefreshAdapter::default();
        let entry = CachedOAuthEntry {
            provider_type: "codex".to_string(),
            auth_header_name: "authorization".to_string(),
            auth_header_value: "Bearer refreshed-access-token".to_string(),
            expires_at_unix_secs: Some(u64::MAX),
            metadata: None,
            source_fingerprint: None,
        };
        let auth = adapter
            .resolve_cached(&sample_transport(), &entry)
            .expect("auth_config authorization header should override cache");

        assert_eq!(
            auth,
            LocalResolvedOAuthRequestAuth::Header {
                name: "authorization".to_string(),
                value: "Bearer imported-session".to_string(),
            }
        );
    }

    #[test]
    fn rejects_cached_bearer_and_metadata_from_replaced_credential_generation() {
        let adapter = GenericOAuthRefreshAdapter::default();
        let mut original = sample_transport();
        original.key.decrypted_api_key = "access-a".to_string();
        original.key.decrypted_auth_config = Some(
            json!({
                "provider_type": "codex",
                "refresh_token": "refresh-a",
                "expires_at": 1,
                "updated_at": 100,
            })
            .to_string(),
        );
        let entry = CachedOAuthEntry {
            provider_type: "codex".to_string(),
            auth_header_name: "authorization".to_string(),
            auth_header_value: "Bearer cached-access-a".to_string(),
            expires_at_unix_secs: Some(u64::MAX),
            metadata: Some(json!({
                "provider_type": "codex",
                "refresh_token": "rotated-refresh-a",
                "expires_at": u64::MAX,
                "updated_at": 200,
            })),
            source_fingerprint: Some(generic_oauth_transport_source_fingerprint(&original)),
        };

        let mut replacement = original.clone();
        replacement.key.decrypted_api_key = "access-b".to_string();
        replacement.key.decrypted_auth_config = Some(
            json!({
                "provider_type": "codex",
                "refresh_token": "refresh-b",
                "expires_at": 1,
                "updated_at": 300,
            })
            .to_string(),
        );

        assert!(adapter.resolve_cached(&replacement, &entry).is_none());
        assert_eq!(
            adapter.base_auth_config(&replacement, Some(&entry)),
            replacement
                .key
                .decrypted_auth_config
                .as_deref()
                .and_then(|value| serde_json::from_str(value).ok())
        );
        assert_eq!(
            current_access_token(&replacement, Some(&entry)).as_deref(),
            Some("access-b")
        );
    }

    #[test]
    fn reuses_cached_bearer_from_matching_credential_generation() {
        let adapter = GenericOAuthRefreshAdapter::default();
        let mut transport = sample_transport();
        transport.key.decrypted_api_key = "access-a".to_string();
        transport.key.decrypted_auth_config = Some(
            json!({
                "provider_type": "codex",
                "refresh_token": "refresh-a",
                "expires_at": 1,
            })
            .to_string(),
        );
        let entry = CachedOAuthEntry {
            provider_type: "codex".to_string(),
            auth_header_name: "authorization".to_string(),
            auth_header_value: "Bearer refreshed-access-a".to_string(),
            expires_at_unix_secs: Some(u64::MAX),
            metadata: Some(json!({
                "provider_type": "codex",
                "refresh_token": "rotated-refresh-a",
                "expires_at": u64::MAX,
            })),
            source_fingerprint: Some(generic_oauth_transport_source_fingerprint(&transport)),
        };

        assert_eq!(
            adapter.resolve_cached(&transport, &entry),
            Some(LocalResolvedOAuthRequestAuth::Header {
                name: "authorization".to_string(),
                value: "Bearer refreshed-access-a".to_string(),
            })
        );
        assert_eq!(
            current_access_token(&transport, Some(&entry)).as_deref(),
            Some("refreshed-access-a")
        );
    }

    #[tokio::test]
    async fn antigravity_expired_legacy_credential_refreshes_and_normalizes_refresh_token() {
        let mut transport = sample_transport();
        transport.provider.name = "Antigravity".to_string();
        transport.provider.provider_type = "antigravity".to_string();
        transport.key.decrypted_api_key = "stale-access-token".to_string();
        transport.key.expires_at_unix_secs = Some(1);
        transport.key.decrypted_auth_config = Some(
            json!({
                "provider_type": "antigravity",
                "refreshToken": "stable-refresh-token",
                "expires_at": 1,
            })
            .to_string(),
        );
        let hits = Arc::new(AtomicUsize::new(0));
        let executor = StaticTokenExecutor {
            hits: Arc::clone(&hits),
        };
        let adapter = GenericOAuthRefreshAdapter::default()
            .with_token_url_for_tests("antigravity", "https://oauth.example/token")
            .with_oauth_credentials_for_tests(
                "antigravity",
                "test-client-id",
                "test-client-secret",
            );

        assert!(adapter.supports(&transport));
        assert!(adapter.should_refresh(&transport, None));

        let refreshed = adapter
            .refresh(&executor, &transport, None)
            .await
            .expect("antigravity refresh should succeed")
            .expect("antigravity refresh should return a cache entry");

        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert_eq!(refreshed.provider_type, "antigravity");
        assert_eq!(refreshed.auth_header_value, "Bearer fresh-access-token");
        assert_eq!(
            refreshed
                .metadata
                .as_ref()
                .map(|metadata| &metadata["refresh_token"]),
            Some(&json!("stable-refresh-token"))
        );
    }

    #[tokio::test]
    async fn fenced_force_reuses_successor_when_refresh_token_does_not_rotate() {
        let mut transport = sample_transport();
        transport.key.decrypted_api_key = "stale-access-token".to_string();
        transport.key.expires_at_unix_secs = Some(u64::MAX);
        transport.key.decrypted_auth_config = Some(
            json!({
                "provider_type": "codex",
                "refresh_token": "stable-refresh-token",
                "expires_at": u64::MAX,
                "headers": {"Authorization": "Bearer stale-top-level"},
                "request": {
                    "extraHeaders": {"authorization": "Bearer stale-request"}
                },
                "transport": {
                    "extra_headers": {"AUTHORIZATION": "Bearer stale-transport"}
                }
            })
            .to_string(),
        );
        let hits = Arc::new(AtomicUsize::new(0));
        let coordinator = LocalOAuthRefreshCoordinator::with_adapters_for_tests(vec![Arc::new(
            GenericOAuthRefreshAdapter::default()
                .with_token_url_for_tests("codex", "https://oauth.example/token"),
        )]);
        let executor = StaticTokenExecutor {
            hits: Arc::clone(&hits),
        };
        let expected = coordinator
            .refresh_fingerprint_for_transport(&transport)
            .expect("refreshable transport should have a generation fence");

        let first = coordinator
            .force_refresh_with_result_fenced(
                &executor,
                &transport,
                None,
                None,
                Some(expected.as_str()),
            )
            .await
            .expect("first refresh should succeed")
            .expect("first refresh should resolve");
        let first_entry = first
            .refreshed_entry
            .as_ref()
            .expect("first refresh should return a cache entry");
        assert_eq!(first_entry.auth_header_value, "Bearer fresh-access-token");
        let metadata = first_entry
            .metadata
            .as_ref()
            .expect("generic refresh should preserve auth metadata");
        assert_eq!(metadata["refresh_token"], "stable-refresh-token");
        assert_eq!(
            metadata["headers"]["Authorization"],
            "Bearer fresh-access-token"
        );
        assert_eq!(
            metadata["request"]["extraHeaders"]["authorization"],
            "Bearer fresh-access-token"
        );
        assert_eq!(
            metadata["transport"]["extra_headers"]["AUTHORIZATION"],
            "Bearer fresh-access-token"
        );
        coordinator
            .store_cached_entry(&transport.key.id, first_entry.clone())
            .await;

        let follower = coordinator
            .force_refresh_with_result_fenced(
                &executor,
                &transport,
                None,
                None,
                Some(expected.as_str()),
            )
            .await
            .expect("follower should reuse the completed refresh")
            .expect("follower should resolve");
        assert!(follower.reused_refresh);
        assert_eq!(
            follower
                .refreshed_entry
                .as_ref()
                .map(|entry| entry.auth_header_value.as_str()),
            Some("Bearer fresh-access-token")
        );
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }
}
